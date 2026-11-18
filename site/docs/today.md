---
title: Plan Your Day with Today
nav: Today
summary: Plan from an inbox of cards beside today's schedule, and prepare for meetings alongside their notes.
reviewed: 0.179.0
---

## Open Today {#open-today}

Click **Today** in the sidebar or press <kbd>0</kbd>. To start there every time, choose **Today** under **Settings → Appearance → Start With**. Otherwise the app remembers the view you last used.

<ts-shot scene="today" alt="Today with the day's plan and its sources on the left and a time grid of today on the right."></ts-shot>

Today has two columns: a **Planning Inbox** of cards on the left, and a larger **Today’s Schedule** on the right. They fill the space below the header, with the schedule scrolling inside its card.

## Read Your Day {#read-your-day}

Use **Previous Day** and **Next Day** to browse the schedule, or **Today** to return to the current date. **New Event** opens the calendar form on the displayed date. The schedule needs a [Google Calendar connection](calendar.html#connect-google-calendar).

- The day view shows today as a time grid with the **Now** line. It opens on your working hours (the **Default** profile in **Settings → Availability → Profiles**), starting a little before now once the working day has begun, and shades the time outside them; evening hours scroll inside it. Without working hours, it opens on now.
- Finished and declined meetings are dimmed. A short meeting shows its start time after its title.
- Each meeting’s note icon opens its existing note, or adds the meeting to its daily note and opens it. See [Meeting Notes](notes.html#meeting-notes).
- Click a meeting’s title to open it in Calendar. Use its note icon to [prepare alongside its notes](#get-ready-for-meetings).
- In a narrow window, the day view gives way to the list of today's events, which marks overlapping meetings and says when a session will be **Recorded**. Dragging onto the day isn't available there; **Schedule** still works.

## Plan Your Day {#plan-your-day}

The **Planning Inbox** is one list of active, unscheduled items. Items you chose for today appear first; other tasks, emails, and contacts follow. An item appears once even when it is both planned and offered by another source. Scheduled work appears on the calendar. Finished tasks appear in a separate **Completed** card, with checked boxes to reopen them. The card includes today's saved task and note-task completions, so refreshing or reopening Today keeps the history.

- Select **New Task** to create a task directly in the Planning Inbox.
- Check a task off on its card. **Completed** appears under **All** and **Tasks**; **Emails** shows completed email tasks. Click a card to open its task, note, conversation, or contact.
- Check an email suggestion off to record its work as completed. A starred conversation opens the task form first; **Cancel** leaves it alone. A promise creates a task linked to its sent message and completes that task. Neither flow changes the email's read, star, or archive state, adds it to the plan, or books time. Completed work from the current message stops being suggested, and a later message can offer new work.
- The **Schedule** icon reserves the next free half hour today, on the quarter hour, within your working hours (the **Default** profile in **Settings → Availability → Profiles**) and around meetings. If no half hour is left, Today says so; if the calendar couldn't be read, free time is unknown. **Undo** removes the block and returns the item to the inbox.
- The card’s **⋯** menu holds secondary actions: **Add to Today's Plan**, **Remove from Today's Plan**, or, for a promise, **Make a Task** and **Dismiss**. Removing an item from the plan keeps its source task, email, or contact.

The plan remains the **Plan** section of today's daily note in [Notes](notes.html). Planning something never changes its due date, and hiding a scheduled card never completes or deletes its source.

### Add to the Plan {#add-to-the-plan}

Use four filters above the list:

- **All** combines the other three categories, with each item appearing once.
- **Tasks** includes open tasks, including goal tasks and tasks in any note. Square `[ ]` checklists stay local to their notes.
- **Emails** includes starred conversations and promises not yet turned into tasks. A conversation that already has an open task appears under **Tasks**. Scheduling a starred conversation opens its task form first; **Cancel** writes nothing and books no time.
- **Contacts** includes people due for a check-in.

The count follows the active filter and counts only unscheduled items. **Completed** has its own count. A plus means more items can be read with **Show More**. Arrow keys, Home, and End move between filters, and Today remembers your choice. Refresh preserves the chosen filter and expanded list.

While viewing today, drag a card onto the schedule to reserve half an hour at that time, including outside working hours when you choose to. Scheduling also adds the item to today’s plan. A contact card books a check-in, without creating a task. If the plan can't be changed, the time stays booked, and **Open Note** lets you resolve the note. **Schedule** offers the same booking flow from the keyboard.

## Get Ready for Meetings {#get-ready-for-meetings}

Use the note icon on a meeting in **Today’s Schedule** to open its existing note, or start its entry in the daily note. Press <kbd>p</kbd> on Today for the meeting in focus, or the next meeting if none is focused. The note opens at the meeting’s link.

- In the note’s **Links** section, the **Prepare Meeting** icon opens [preparation](calendar.html#prepare-for-a-meeting) in the side panel. The editor stays visible, so you can write while reviewing previous notes, related conversations, and commitments.
- **Close Preparation** or <kbd>Escape</kbd> returns to note details. If you open a source, **Back to Meeting** restores the original note and its preparation.
- **Reserve Prep Time** adds a task, "Prepare for" and the meeting's name, under the meeting's line in that day's note, adding the line if the note doesn't have one. It appears in **Tasks**. Doing it again finds the task already there.
- Working locations, such as **Home** or **Office**, aren't meetings, so they offer no **Prepare** or **Add Note**; clicking one opens it in Calendar.

Email and ordinary task candidates follow the selected mail account. The schedule shows all selected calendars. **Open Calendar** opens the full calendar at the date you’re viewing.

## You Said You Would {#you-said-you-would}

In the **Emails** filter, **You Said You Would** finds promises in your own sent mail from the last two weeks, such as “I'll send the report over,” when you have not written to that person since. Click one to open its conversation, turn it into a task, add it to your plan, or dismiss it. Detection runs on your computer without AI. Review the suggestion; it may not know you already finished the work elsewhere.
