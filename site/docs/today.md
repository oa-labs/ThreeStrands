---
title: Plan Your Day with Today
nav: Today
summary: Plan from an inbox of cards beside today's schedule, and prepare for meetings alongside their notes.
reviewed: 0.179.0
---

## Open Today {#open-today}

Click **Today** in the sidebar or press <kbd>0</kbd>. To start there every time, choose **Today** under **Settings → Appearance → Start With**. Otherwise the app remembers the view you last used.

<ts-shot scene="today" alt="Today with the day's plan and its sources on the left and a time grid of today on the right."></ts-shot>

Today has two columns: a **Planning Inbox** of cards on the left, and a larger **Today’s Schedule** on the right. They fill the space below the header, with the schedule scrolling inside its card. **Day Details** reveals the mail and meeting summaries when you want them. In the evening, Today turns into a [review of the day](#review-your-day) with tomorrow on the right.

## Read Your Day {#read-your-day}

Open **Day Details** for a summary of the day's meetings: "2 meetings · next at 2:00 pm", the meeting you're in and when it ends, or "2 meetings, both finished · last ended 3:30 pm", along with any double-booking still ahead. Once your plan has items, the next line says how many are done, such as "3 of 5 planned done". The day view needs a [Google Calendar connection](calendar.html#connect-google-calendar).

- The day view shows today as a time grid with the **Now** line. It opens on your working hours (the **Default** profile in **Settings → Availability → Profiles**), starting a little before now once the working day has begun, and shades the time outside them; evening hours scroll inside it. Without working hours, it opens on now.
- Finished and declined meetings are dimmed. A short meeting shows its start time after its title.
- **Review Today** shows tomorrow’s schedule.
- **Join**, under **Day Details**, opens your next online meeting.
- Each meeting’s note icon opens its existing note, or adds the meeting to its daily note and opens it. See [Meeting Notes](notes.html#meeting-notes).
- Click a meeting’s title to open it in Calendar. Use its note icon to [prepare alongside its notes](#get-ready-for-meetings).
- In a narrow window, the day view gives way to the list of today's events, which marks overlapping meetings and says when a session will be **Recorded**. Dragging onto the day isn't available there; **Schedule** still works.

## Review Your Day {#review-your-day}

In the evening, Today shows what the day came to and what is left over, with tomorrow beside it. It opens by itself once your last meeting has ended and your working hours are over (the **Default** profile in **Settings → Availability → Profiles**, or 6 pm without working hours). Earlier, click **Review Today** in the header, or choose **Review Today** in the [command palette](keyboard-shortcuts.html), <kbd>⌘/Ctrl + k</kbd>, which switches between the two. **Back to Today's Plan** returns to the day. The view you choose lasts for the rest of the day on this computer. Nothing reminds you to use it.

**Today in review**, on the left:

- **Done today**: the plan items you finished, the meetings you attended, and other tasks you finished today. Click one to open it.
- **Loose ends**: **Unfinished planned items**, each with **Carry to Tomorrow**, **Defer…** (pick a date from tomorrow on in a small window), and **Drop**. Each choice is saved to today's note at once, and **Undo** in the notice takes it back. Anything you leave alone comes back in tomorrow morning's review. Below them, the day's finished meetings show whether you reviewed their follow-through (**Review Follow-through**, or **No Follow-through** to set one aside on this computer), the promises you made in today's sent mail offer **Make a Task** or **Dismiss**, and one line counts the mail that needs a reply and opens your inbox.
- **Notes on the day**: **Add Notes on the Day…** opens a small window; what you write is added as a paragraph under **Shutdown** at the end of the Plan section of today's note, with **Undo**.

The header shows **Day closed** when every finished meeting has been reviewed or set aside and no planned item is left undecided. It follows what is saved in your notes, so it agrees on your other computers once they sync; only **No Follow-through** stays on one computer.

**Tomorrow**, on the right, lists what you carried to tomorrow, then tomorrow's calendar as a day view with your working hours shaded. A meeting’s title opens Calendar, and its note icon opens its notes for [preparation](#get-ready-for-meetings). **Plan Tomorrow** opens [Plan Today](#plan-today) for tomorrow with what you carried already chosen.

## Plan Your Day {#plan-your-day}

The **Planning Inbox** is one list of active, unscheduled items. Items you chose for today appear first; other tasks, emails, and contacts follow. An item appears once even when it is both planned and offered by another source. Scheduled work appears on the calendar. Finished tasks appear in a separate **Completed** card, with checked boxes to reopen them. The card includes today's saved task and note-task completions, so refreshing or reopening Today keeps the history.

- Check a task off on its card. **Completed** appears under **All** and **Tasks**; **Emails** shows completed email tasks. Click a card to open its task, note, conversation, or contact.
- Check an email suggestion off to record its work as completed. A starred conversation opens the task form first; **Cancel** leaves it alone. A promise creates a task linked to its sent message and completes that task. Neither flow changes the email's read, star, or archive state, adds it to the plan, or books time. Completed work from the current message stops being suggested, and a later message can offer new work.
- The **Schedule** icon reserves the next free half hour today, on the quarter hour, within your working hours (the **Default** profile in **Settings → Availability → Profiles**) and around meetings. If no half hour is left, Today says so; if the calendar couldn't be read, free time is unknown. **Undo** removes the block and returns the item to the inbox.
- The card’s **⋯** menu holds secondary actions: **Add to Today's Plan**, **Remove from Today's Plan**, or, for a promise, **Make a Task** and **Dismiss**. Removing an item from the plan keeps its source task, email, or contact.

The plan remains the **Plan** section of today's daily note in [Notes](notes.html). Planning something never changes its due date, and hiding a scheduled card never completes or deletes its source.

### Plan Today {#plan-today}

**Plan Today** in Today's header (or in the [command palette](keyboard-shortcuts.html), <kbd>⌘/Ctrl + k</kbd>) walks you through the day. Once your plan has items, the header shows its progress and **Edit Plan**, which starts from the saved plan. Late in the day, after your last meeting or in the last hour of your working hours, the header also offers **Plan Tomorrow**, which plans tomorrow's note the same way (the [evening view](#review-your-day) has it beside tomorrow); the palette has **Plan Tomorrow** at any time. Items you carried to tomorrow start out chosen as **Carry** in tomorrow's review.

Move between the steps with **Next** and **Back**, or jump to one in the step list; every step can be skipped. Nothing is written to your note until you save.

1. **Review**: what you got done from your last plan (finished items, the meetings you attended, and other tasks you finished that day), and what you **Didn't Get To**. For each, choose **Carry** (into this plan), **Defer…** (until a later date), or **Drop**, or **Carry All**. Items you leave undecided come back next time. An item carried three times or more asks whether to defer or drop it, but nothing is decided for you. Without anything to review, the step is skipped.
2. **Choose**: individual sources, including Goals, a search of every open task, and **New Task** to type a task that lives in the plan. Your choices form an ordered list; move items with the arrows or <kbd>Option/Alt + ↑</kbd> and <kbd>Option/Alt + ↓</kbd>. Removing an item that was already in the plan takes its line out of the note. Past five items, Today notes that most people finish three to five.
3. **Meetings**: the day's meetings with what is ready for them, **Reserve Prep Time**, and **No Prep Needed**. **Prepare** opens the meeting's preparation inside Plan Today; **Back to Meetings** returns with your choices as they were.
4. **Fit**: your choices beside the day's calendar, with the free time left in your working hours. **Book** or drag a card onto the day to reserve half an hour. Booking happens right away, with **Undo**, and stays even if you don't save the plan.
5. **Save**: exactly what will be written to the note, and any time already booked. **Save Plan** (or <kbd>⌘/Ctrl + Enter</kbd> from any step) writes it all at once; **Undo** in the notice reverses it if the note hasn't changed since. **Copy Plan** copies the plan as plain text.

Carried items are marked with the day they came from, and deferred or dropped ones are listed under **Not today** in the note, so the next review knows they are settled. Press <kbd>Escape</kbd> to close: Plan Today asks whether to keep your choices for later or discard them. Kept choices, and choices left when the app closes, stay on this computer for a week and return the next time you plan that day, with **Discard** to start over.

### Add to the Plan {#add-to-the-plan}

Use four filters above the list:

- **All** combines the other three categories, with each item appearing once.
- **Tasks** includes open tasks, including goal tasks and tasks in any note. Square `[ ]` checklists stay local to their notes.
- **Emails** includes starred conversations and promises not yet turned into tasks. A conversation that already has an open task appears under **Tasks**. Scheduling a starred conversation opens its task form first; **Cancel** writes nothing and books no time.
- **Contacts** includes people due for a check-in.

The count follows the active filter and counts only unscheduled items. **Completed** has its own count. A plus means more items can be read with **Show More**. Arrow keys, Home, and End move between filters, and Today remembers your choice. Refresh preserves the chosen filter and expanded list.

Drag a card onto the schedule to reserve half an hour at that time, including outside working hours when you choose to. Scheduling also adds the item to today’s plan. A contact card books a check-in, without creating a task. If the plan can't be changed, the time stays booked, and **Open Note** lets you resolve the note. **Schedule** offers the same booking flow from the keyboard.

## Get Ready for Meetings {#get-ready-for-meetings}

Use the note icon on a meeting in **Today’s Schedule** to open its existing note, or start its entry in the daily note. Press <kbd>p</kbd> on Today for the meeting in focus, or the next meeting if none is focused. The note opens at the meeting’s link.

- In the note’s **Links** section, the **Prepare Meeting** icon opens [preparation](calendar.html#prepare-for-a-meeting) in the side panel. The editor stays visible, so you can write while reviewing previous notes, related conversations, and commitments.
- **Close Preparation** or <kbd>Escape</kbd> returns to note details. If you open a source, **Back to Meeting** restores the original note and its preparation.
- **Reserve Prep Time** adds a task, "Prepare for" and the meeting's name, under the meeting's line in that day's note, adding the line if the note doesn't have one. It appears in **Tasks**. Doing it again finds the task already there.
- Working locations, such as **Home** or **Office**, aren't meetings, so they offer no **Prepare** or **Add Note**; clicking one opens it in Calendar.

## Keep an Eye on Mail {#keep-an-eye-on-mail}

Under **Day Details**, the **Mail** line lists calendar invitations by name, since they need an answer in Calendar, and counts what **needs a reply** (messages written to you and at most two others by people you write to) and the rest of your unread mail. Click an invitation to open it, or **Open Inbox** for everything else. The arrow before **Mail** folds the line to a short summary; Today remembers that on this computer.

Mail, email candidates, and ordinary task candidates follow the selected mail account, named above the inbox when several accounts are configured. Contacts and note tasks include all accounts. Saved plan items also remain visible across accounts. The plan and the day view cover every account: the day view includes every calendar you show in **Calendar**. If it looks wrong, check [calendar selection](troubleshooting.html#calendar-availability-looks-wrong). **Open Calendar** leads to the full workspace.

## You Said You Would {#you-said-you-would}

In the **Emails** filter, **You Said You Would** finds promises in your own sent mail from the last two weeks, such as “I'll send the report over,” when you have not written to that person since. Click one to open its conversation, turn it into a task, add it to your plan, or dismiss it. Detection runs on your computer without AI. Review the suggestion; it may not know you already finished the work elsewhere.
