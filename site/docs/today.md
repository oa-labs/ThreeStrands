---
title: Plan Your Day with Today
nav: Today
summary: Plan from an inbox of tasks, emails, and contacts beside the day's agenda or daily note, and prepare for meetings alongside their notes.
reviewed: 0.210.0
---

## Open Today {#open-today}

Click **Today** in the sidebar or press <kbd>0</kbd>. To start there every time, choose **Today** under **Settings → Appearance → Start With**. Otherwise the app remembers the view you last used. For an optional sky behind the header that follows the time of day, see [Add a Backdrop to Today](appearance.html#today-backdrop).

<ts-shot scene="today" alt="Today with the Planning Inbox of tasks, emails, and contacts on the left and the day's agenda on the right."></ts-shot>

Today has two columns: the **Planning Inbox** on the left, with a **Done Today** card below it, and a larger **Agenda** on the right. Press <kbd>Tab</kbd> or <kbd>Shift + Tab</kbd>, or click the hint beside the date controls, to switch the right column between the **Agenda** and that day's **Daily Note**. While you're writing in the note, press <kbd>Escape</kbd> first. Pull down on Today to refresh it; your filter, selection, and unfinished edits stay as they were.

## Read Your Day {#read-your-day}

Use **Previous Day** and **Next Day** beside the date, or press <kbd>-</kbd> and <kbd>=</kbd>, to browse other days, and **Today** to return. The date controls stay in the same place on every day, and Today keeps the day you picked until midnight. **Open Calendar** opens the full calendar at that date. The agenda needs a [Google Calendar connection](calendar.html#connect-google-calendar).

- The agenda shows the day as a time grid with a **Now** line. It opens on your working hours (the **Default** profile in **Settings → Availability → Profiles**), starting a little before now once the working day has begun, and shades the time outside them. Without working hours, it opens on now. When meetings are scrolled out of view above or below, a small button takes you to them.
- Finished and declined meetings are dimmed. A short meeting shows its start time after its title. A meeting that starts well after another has begun sits on top of it, so both keep readable titles.
- Your Google Calendar working location, such as *Office*, isn't a meeting, so it doesn't take up the agenda or count as one.
- Click a meeting’s title to open it in Calendar. Each meeting’s note icon opens its existing note, or adds the meeting to its daily note and opens it. See [Meeting Notes](notes.html#meeting-notes) and [Get Ready for Meetings](#get-ready-for-meetings).
- Time you booked for a task shows a task button that opens the task.
- In a narrow window, the agenda gives way to a list of the day's events, which marks overlapping meetings and says when a session will be **Recorded**. Dragging onto the day isn't available there; **Schedule** still works.

### Add Events {#add-events}

Click or drag on the agenda to create an event at that time, or click **+** beside the date controls. Either opens the calendar form on the day you're viewing.

## Plan Your Day {#plan-your-day}

The **Planning Inbox** is one list of active, unscheduled work. Items you chose for today come first; other tasks, emails, and contacts follow. An item appears once even when it is both planned and offered by another source. Scheduled work moves to the agenda without completing its source. Task actions appear when you point at a task, and overdue dates are marked.

- Type in the **Add a task** field at the end of the list and press <kbd>Enter</kbd> to create a task.
- Check a task off on its card. Click a card to open its task, note, conversation, or contact.
- Check an email suggestion off to record its work as a completed task. A starred conversation opens the task form first; **Cancel** leaves it alone. A promise creates a task linked to its sent message and completes that task. Neither flow changes the email's read, star, or archive state, adds it to the plan, or books time. Completed work from the current message stops being suggested, and a later message can offer new work.
- The **Schedule** icon reserves the next free half hour today, on the quarter hour, within your working hours and around meetings. If no half hour is left, Today says so; if the calendar couldn't be read, free time is unknown. **Undo** removes the block and returns the item to the inbox.
- The card’s **⋯** menu holds secondary actions: **Add to Today's Plan**, **Remove from Today's Plan**, or, for a promise, **Make a Task** and **Dismiss**. Removing an item from the plan keeps its source task, email, or contact.

The plan remains the **Plan** section of today's daily note in [Notes](notes.html). Planning something never changes its due date, and hiding a scheduled card never completes or deletes its source.

### Filter the Inbox {#add-to-the-plan}

Use the filters above the list:

- **All** combines tasks, emails, and contacts, with each item appearing once.
- **Tasks** includes open tasks, including goal tasks and tasks in any note. Square `[ ]` checklists stay local to their notes.
- **Emails** includes starred conversations and promises not yet turned into tasks. A conversation that already has an open task appears under **Tasks**. Scheduling a starred conversation opens its task form first; **Cancel** writes nothing and books no time.
- **Contacts** includes people due for a check-in, and says when you're up to date with your keep-in-touch goals.
- **Commitments** shows recent promises, tracked commitments, follow-ups due, and possible replies to starred conversations. Open a row for its source or task. Use **⋯** to draft a reply or dismiss a suggestion. Dismissal is device-local and applies to that message. Reply suggestions can use a Brief you already ran and work with local mail even when AI is off.

The count follows the active filter. Work with time already booked lives on the agenda. A plus means more items can be read with **Show More**. Arrow keys, Home, and End move between filters, and Today remembers your choice.

### Book Time by Dragging {#book-time-by-dragging}

Drag a card onto the agenda to reserve half an hour at that time, on today or a future day you've browsed to, including outside working hours when you choose to. A small label follows the pointer while you drag, and <kbd>Escape</kbd> cancels. Scheduling also adds the item to that day's plan. A contact card books a check-in without creating a task. If the plan can't be changed, the time stays booked, and **Open Note** lets you resolve the note. **Schedule** offers the same booking from the keyboard.

### Done Today {#done-today}

The **Done Today** card under the Planning Inbox lists what you finished on the day the agenda shows: tasks you completed, emails you sent, notes you created, and starred notes you edited. Its title reads **Done** on other days. It follows the same filters as the inbox, and you can reopen a task by unchecking it. When you check something off, its title briefly carries over into the card.

## Get Ready for Meetings {#get-ready-for-meetings}

Use the note icon on a meeting in the **Agenda** to open its existing note, or start its entry in the daily note. Press <kbd>p</kbd> on Today for the meeting in focus, or the next meeting if none is focused. The note opens at the meeting’s link.

- In the note’s **Links** section, the **Prepare Meeting** icon opens [preparation](calendar.html#prepare-for-a-meeting) in the side panel. The editor stays visible, so you can write while reviewing previous notes, related conversations, and commitments.
- **Close Preparation** or <kbd>Escape</kbd> returns to note details. If you open a source, **Back to Meeting** restores the original note and its preparation.
- **Reserve Prep Time** adds a task, "Prepare for" and the meeting's name, under the meeting's line in that day's note, adding the line if the note doesn't have one. It appears in **Tasks**. Doing it again finds the task already there.
- Working locations, such as **Home** or **Office**, aren't meetings, so they offer no **Prepare** or **Add Note**; clicking one opens it in Calendar.

Email and ordinary task candidates follow the selected mail account. The agenda shows all selected calendars.

## You Said You Would {#you-said-you-would}

In the **Emails** filter, **You Said You Would** finds promises in your own sent mail from the last two weeks, such as “I'll send the report over,” when you have not written to that person since. Click one to open its conversation, turn it into a task, add it to your plan, or dismiss it. Detection runs on your computer without AI. Review the suggestion; it may not know you already finished the work elsewhere.
