# Changelog

What's new in ThreeStrands, written for the people who use it. Each release's notes on GitHub and the changelog page on the website come from this file.

## Unreleased

## 0.210.0 — 2026-11-12

### New
- **Notes** now has a calendar at the top of its side panel. Choose any day to open its daily note; a dot marks each day you've written in. On a daily note, the panel also lists that day's events, and **Add Note** on one adds the meeting to the note you're reading.
- Step through daily notes with the arrows beside the date, or with <kbd>-</kbd> and <kbd>=</kbd>, as in **Calendar**. <kbd>t</kbd> opens today's note.
- When you open an invitation, the panel beside the conversation shows **That day**: everything else on your calendar that day, whether the invitation's time is free, and which events it overlaps. Your Google Calendar working location, such as *Office*, appears once beside the date rather than as an event. Pressing **t** now opens the schedule on the invitation's day instead of today.

### Improved
- **Availability** is now its own group in Settings, with separate pages for **Profiles**, **Account Defaults**, and **Meeting Types**. Profiles lists each profile's hours at a glance and marks the default, keeps timezone and slot increment under **Shared Settings**, and calls working hours **Available Hours**. Meeting Types has an **Edit Profile** link to change a profile's hours.
- Choose 75 minutes as the default meeting duration in **Settings → Availability**.
- **Edit Event** now makes recurring-event edits explicit as applying to one occurrence, shows times in a selectable time zone, and groups scheduling controls with the meeting duration. Larger guest lists collapse into a compact summary, and event time fields offer quarter-hour suggestions while accepting exact typed times.
- Click **Import Note** beside **New Note** to add a Markdown file and open it for writing. Notes with the same title get a numbered name, so your existing notes are kept.
- **Calendar** and the schedule beside your mail show your Google Calendar working location, such as *Office* or *Home*, as a place marker above the day's all-day events instead of as an event, once even when several calendars share it. A location set for part of a day appears there with its hours rather than as a block of time.
- Recipient suggestions prefer people you have recently emailed more than once
  together with several of the addresses already on the message, not only
  people you email often with each of them separately. When you add members to
  a group, a member's several addresses count as one person, so someone you
  email only with that member is no longer ranked as if they knew two.
- The mail search box has more room, with saved-search buttons aligned closer to the right edge of the email cards.
- Associate availability profiles with lunch, breakfast, dinner, or coffee invitations in **Settings → Availability → Meeting Types**. Find Availability uses those hours and shows the selected profile above the time controls, with a choice to change it for the conversation or draft.
- A note's text now starts closer under its title, so the two read as one page.
- Notes now uses the same shortcuts as email for fixed-width text, quotes, and code blocks, alongside its existing bold, italic, link, and list shortcuts.
- Select recipient badges in **To**, **Cc**, or **Bcc** by clicking them, with the arrow keys, or with <kbd>⌘/Ctrl + a</kbd>, then cut, copy, paste, delete, or drag them between fields together.
- Drag the right edge of the note list in **Notes** to make it wider or narrower, as you can with the lists in **Mail** and **Contacts**.
- Every **Settings** page now has the same layout: it opens with a section heading at the same spacing, and its explanations use the same text size.
- The side panel in **Mail**, **Contacts**, **Notes**, **Files**, **Tasks**, and **Calendar** now resizes from its left edge. Drag it past that edge to hide it, or press <kbd>\</kbd>; the button on its edge, or <kbd>\</kbd> again, brings it back. Each view remembers whether you hid it, and the width you choose applies everywhere.
- Suggested and proposed meeting times now show the day around them on your
  calendar, including short-break warnings, so you can see what you have before
  and after without opening Calendar.

### Fixed
- Removing a mail account now deletes everything ThreeStrands kept for it on your computer, including its drafts, messages waiting in the **Outbox**, attachments you opened, and older backup copies of your mail. **Remove on all devices** clears the same from your other devices.
- Clicking the message body shows the text cursor, including right after attaching a file.
- Event details now offers **Open Note** when the meeting already has a linked note. Calendar note actions open that note instead of offering to add the meeting again.
- Names, email addresses, and subjects that **Add Note**, **Add to Note**, and contact links write into a note no longer show stray backslashes, such as `kim\.peters@wolfe\.com`.
- Pasting formatted text from an email or web page into a note no longer leaves codes such as `&#x20;` and `&#xA0;` where spaces were.
- **Backlinks** in Notes now show the linking line as you'd read it, such as "3:00 pm met with Dana for OA Monthly", instead of its raw link text.
- In **Settings**, the "Changes save automatically" note and the **Help** link now line up at the top of each page.
- Emails that set a background image on a table or table cell, such as a photo behind a "Help center" panel, now show that image when you load images, as other mail apps do.

## 0.209.0 — 2026-11-10

### New
- **Add Note** on a calendar event, in **Calendar** and **Today**, writes a line about the meeting under **Meetings** in that day's note, linking the event and each attendee's contact. Check **Create backlinked note** to also link a note named after the meeting, which every later meeting with that name shares. A contact's details now list the notes that mention them.
- In Notes, type `@` to find and link a contact or email conversation by name, address, subject, or participant. Contact links open the contact record, conversation links open the email, and `[[` remains the shortcut for linking notes.

### Improved
- A **Brief** no longer suggests adding an event that's already on your calendar under another name. When someone from the conversation is on a calendar event at the same time, the suggestion shrinks to one line with **Open in Calendar**, and it stops warning that your own meeting conflicts. Briefs also stop suggesting tasks to accept or decline an invitation, since you answer those from your calendar.
- Notes keeps your search, search scope, and selected note when you switch workspaces. Its search field now matches Mail, with a magnifying glass and the <kbd>/</kbd> shortcut; <kbd>Enter</kbd> returns to the results while keeping your search, and <kbd>Escape</kbd> clears it and returns to navigation.
- Notes search excludes Trash by default, offers **Include Trash**, and searches only trashed notes while you are in **Trash**.
- Note Details gives checklist text more room, shows unfinished items first, and groups completed items behind a count. Use an item’s menu to read its full text or convert between a checklist and a Task.
- The Notes details panel has a matching header icon, a compact action for adding links, and quieter timestamps below related content. Link and completion labels line up with checklist text.
- An event's details in **Calendar** now list, under **Series notes**, the notes from every other meeting in its recurring series, newest first, alongside the notes for that day's meeting.
- **Link to…** now offers contacts alongside conversations, tasks, and events.
- The command palette (<kbd>⌘K</kbd>) now finds your notes too: matching notes are listed below the commands, and choosing one opens it in **Notes**.
- In **Notes**, <kbd>Space</kbd> and <kbd>Shift</kbd>+<kbd>Space</kbd> page through the open note, as they do for a message, and <kbd>Enter</kbd> starts writing in it.
- Use <kbd>↑</kbd> and <kbd>↓</kbd> to switch between notes even when the note list is not focused. Arrow keys still move the cursor while editing.
- The side panel now opens with the name of the person you're reading or writing to, with their role and company (or their email address) and any notes you saved. **Find Times** and **Delegate Work…** sit right below.
- **Recent emails** now comes before **Files**, subjects drop their "Re:" and "Fwd:" markers, and lists show up to five items before **Show more**. Item counts in section headings no longer look like keyboard shortcuts.
- A file's **Show email** and star buttons now appear when you point at the file. A starred file keeps its star.

### Fixed
- Due dates on suggested tasks now read like the rest of your tasks, such as "Tomorrow, 11:00 AM", instead of showing raw dates and times.
- **Suggestions** now have their own section below **Brief** in the side panel, with **Get Suggestions** on its heading. Collapsing **Brief** folds just the brief, and each section remembers whether it is collapsed.
- Wide emails now fit the reader automatically without horizontal scrolling or clipped content, including when Email Zoom is enlarged.
- Note titles and text stay in place when switching between empty and saved daily notes.

## 0.208.0 — 2026-11-09

### New
- Add conversations to today's note and link notes to conversations, tasks, and calendar events, with links back to your notes in each item's context. Round tasks in notes appear in **Tasks** and can be checked in either place; square checklists stay in their notes.
- **Notes** (<kbd>4</kbd>) keeps a note for each day and notes of your own, formatted as you write in Markdown, linked with `[[Title]]` (each note lists the notes that link to it), saved as you type, holding pictures and files you paste or drag in (which **Files** lists too), kept in Trash until you delete them, and imported from a Reflect folder or exported to a folder of Markdown files from **Settings → Notes**. With cross-device sync on, notes reach your other devices, and a note changed on two devices at once lets you compare the versions and choose; update every device before you start writing.
- Save a mail search with a name, one account or All accounts, **Inbox Only**, and quick filters, then open it as a tab alongside your Split Inboxes without routing mail out of Main. Create, rename, edit, reorder, or delete saved views in **Settings → Split Inboxes**; existing saved searches appear automatically and stay on this computer.

### Improved
- Dates in Mail, contact history, and Files now use consistent month and day labels, with the year included for older dates. File date filters always show the year, while detail timestamps retain the time.
- Formatted emails now use the same line spacing as other mail clients unless the sender sets their own, so short notes and signatures are no longer spread out. Plain-text emails keep their current spacing.
- Create and apply labels directly from **Manage Labels** on Gmail IMAP accounts, including nested and non-English names.
- Mail search leaves more room for your query with compact Inbox and Clear controls. Save Search now sits beside Saved searches outside the input.
- Get Brief now lines up with Find Times when the conversation actions wrap onto another row, with matching icon sizes.
- The view shortcuts follow the sidebar order: <kbd>5</kbd> now opens Contacts, <kbd>6</kbd> Files, and <kbd>7</kbd> Agents.
- Mail search now shows up to five entries under **Recent searches** and remembers searches you submit, save, or use to open a conversation, rather than recording unfinished typing when focus moves away. The close button sits closer to the search box’s right edge.
- The sidebar is easier to scan, with more space below Compose and a smaller gap before Files and Agents.
- Dialog titles, buttons, and commands now use the same name and capitalization for the same action. For example, scheduling uses **Send Later** throughout, retry buttons all say **Try Again**, and the command palette lists **New Event** and **New Group** under the names the toolbars show.
- Keyboard shortcuts now look the same everywhere in the app, written the way your platform writes them: ⇧⌘L on a Mac, Ctrl+Shift+L on Windows and Linux.
- Status notices, the Outbox batch actions, and the names screen readers announce for icon-only buttons now use consistent punctuation and capitalization.

### Fixed
- Emails written in Outlook or Word no longer show extra blank space between lines or paragraphs. Their paragraph spacing, text sizes, and table padding now appear as the sender set them.
- Composing an email now highlights the pencil icon. Sidebar navigation works while composing and saves your draft before switching views.
- Editing a note’s title now pauses pending automatic saves, so unfinished names do not get saved or rewrite links in other notes. Leaving the title still saves the completed rename, including when a body save is already in progress.
- Exporting and importing Notes now preserves note titles and the links between them, including titles with special characters or long Unicode text. A leading heading stays in the note when the file has an explicit title.
- Returning to Notes now restores the last note you opened, even after opening a note from Files or jumping to today’s note.
- Notes created with the same title on different devices now receive distinct, consistent titles after syncing. Both notes remain available to open, edit, and link to.
- Icon buttons now come in two consistent sizes, so controls such as the search bar's buttons, account reorder arrows, and **Find a Time**'s week arrows match the buttons around them.
- IMAP accounts with native labels now correctly recognize label names containing quotation marks or backslashes.
- Counts of one now read naturally, such as "1 draft", "1 conversation", and "Imported 1 contact".

## 0.207.1 — 2026-11-06

### Improved
- Adding an event from an email's Brief now picks the primary calendar of the account the email arrived on, instead of a calendar from another account. In **New event**, the **Calendar** list shows each account's primary calendar at the top, apart from the rest.
- As you type a **Location** for a new or edited event, ThreeStrands suggests places from your other calendar events, most-used first. Choose one with the arrow keys and Return, or keep typing your own.
- IMAP search now includes archived mail in All Mail and messages stored only in user folders. On servers with label-based archiving, archiving preserves other labels and no longer requires a separate Archive mailbox.
- Gmail IMAP accounts can now apply and remove existing labels while preserving other labels and keeping messages in All Mail.
- New event and Edit event place **All day** beside the start and end fields, with the checkbox and label together on one line. Dates now open a calendar styled to match the app, with separate time controls.
- **Invite People** on new and edited events now suggests people the way compose does: ranked by how often you email them, with the people you usually email alongside those already invited shown first, marked **Often emailed with**.
- Saved contacts now come before people who are only in your mail history when you type a name, whether you're writing an email or inviting people to an event. They're also found by the name you saved them under, even if their emails use a different one.
- **Settings → Reading & Triage** now lets you choose to mark conversations read **Immediately**, **After a Delay**, or **Never** when you open them. With **Never**, conversations stay unread until you mark them read yourself.
- **Settings → Reading & Triage** can also mark conversations read when you move them to Trash or snooze them, bring snoozed conversations back as unread, and choose whether archiving opens the next older conversation, the next newer one, or returns to the list.

## 0.207.0 — 2026-11-06

### New
- **Settings → Reading & Triage** brings together when conversations are marked read, including a new option to mark them read when archiving, even without opening them. The archive option starts off, and the reading delay has moved here from Appearance.

### Improved
- **Settings → Diagnostics** shows a failed Gmail sync as one readable line with the error code and Google's message, instead of the raw reply. A failed sync attempt, which clears once the next sync succeeds, no longer counts as an item to review.
- Web research setup has moved from a separate dialog in **Agents** to **Settings → Web Research**. It now shows what is still needed before research can start, and lets you save each key on its own. It no longer asks you to record a storage agreement.
- **Agents** replaces the "Delegated work enabled" checkbox with **Pause All**. While work is paused, a notice says how many tasks are waiting, and **Resume All** restarts only the tasks it paused.
- **Settings** groups its Workflow sections by task: incoming mail first, then writing, then scheduling.

### Fixed
- Logos and other images in emails no longer look squashed or stretched. Small logos keep the size the sender chose, and images that have to shrink to fit the reading pane keep their shape.
- Gmail accounts no longer run into Gmail's per-minute usage limit while syncing, opening messages, and saving drafts at the same time. When Gmail does report the limit, ThreeStrands now pauses that account's requests until the limit resets instead of retrying until sync fails.
  When the same account is signed in on several computers, each one slows down after Gmail reports the limit and speeds back up gradually, so they share the limit instead of taking turns failing.
- IMAP messages with reused sender message IDs stay separate when the server supplies unique identifiers. Their identities remain stable across folder copies, moves, and reconnects.
- Very long conversations that go over ThreeStrands' size or message limit now keep their newest messages and leave out the oldest, instead of cutting off the latest replies.
- **Settings → Diagnostics** lists the messages left out of each conversation as one entry with a count, and no longer shows them as new every time the conversation syncs again.
- A new message started from **All accounts** now keeps the sending account you last used, instead of switching back to your first account whenever ThreeStrands checks for mail.

## 0.205.0 — 2026-11-06

### New
- Starred conversations now appear in an automatic **Starred** inbox tab, including read and archived mail, so follow-ups stay easy to find. The tab hides when empty; you can turn it off in **Settings → Split Inboxes**.
- Delegate read-only web research in **Agents** with separately configured Anthropic and Exa keys: review public search subjects, answer questions, discuss findings, and save report revisions with sources. Research has shared usage limits and explicit data-sharing choices; the local demonstration remains available without external requests.
- When an email offers meeting times, check one and choose **Accept & Add to Calendar**: the new event opens already filled in from the meeting suggestion, and once you save it, a reply confirming the time is drafted for you. The meeting suggestion now points you to **Availability** instead of offering to add an event with no time.

### Improved
- A conversation's **Brief** now starts with where things stand and whose move it is, and ends with what it would take to wrap it up, such as a fix to confirm or a workaround to undo. It calls you "you" even when others mention you by name, leaves out requests someone already answered, and says when a conversation has gone quiet for a few days.
- Meeting suggestions offer times spread across the whole range, mixing mornings and afternoons, instead of the first few days. When the email names days, such as "Tuesday or Thursday", only those days are offered, with **Show All Days** to see the rest. You can change the meeting length beside **Mornings** and **Afternoons**, the reply button counts the times you picked, and **Remind Me** in the ⋯ menu adds a task, due tomorrow, to send times later. Coming back to a conversation shows its times right away, with the ones you picked, instead of checking your calendar again from scratch.
- Suggestions are easier to act on: a suggestion that may be wrong says what to check, such as "Length not stated", and a meeting no longer comes with a second task that asks for the same thing. While a brief is being prepared, the panel says "Looking for suggestions…" instead of showing a button that soon disappears. **Suggested** can be collapsed on its own.
- Primary buttons now use a solid fill, so the main action in a dialog or card stands out in every theme and accent color.
- Check something while you write: when you leave a draft to read another conversation, **Return to Draft** appears above it, and <kbd>g</kbd> then <kbd>c</kbd> takes you back to the draft from anywhere. Press <kbd>⌘/Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>T</kbd> while writing to show your calendar beside the draft.
- Agents now uses consistent text sizes, clearer form grouping, and aligned controls.
- The mail search box leaves more room for your search: the / hint only shows while search is idle, and the Inbox Only filter shrinks to an icon until you turn it on.
- Click an event's address in event details, invitation cards, or meeting suggestions to open it in Google Maps.
- In **Find Times**, click an offered time to see that day below it: the meetings just before and after, each as its own card, where they are, and how much free time is between them. Times with no break, or a short one, beside another meeting are marked **Back-to-back** or **Short break** before you open the day. Use the checkbox beside each time to choose which ones to offer.
- When an email offers meeting times, each one has a checkbox for choosing it, and timezones go by familiar names such as Eastern Time (ET). If the email doesn't say how long the meeting is, your usual length is assumed and can be changed, so the times can still be offered. Times written together, such as "Monday at 2:00 or 3:00 pm", both keep their day and AM/PM.
- **Show 3 More** replaces **Show More**, and adds more times from the same search. **Search Again** is replaced by a **Re-read** link beside the explanation, shown only after the conversation or your draft changes, since reading the same conversation again would find the same times.

### Fixed
- Meeting suggestions name you as "you" among the people, even when the email lists you by name, and no longer warn that the day and time aren't chosen when picking them is what the card is for.
- Restarting ThreeStrands now keeps messages expanded or collapsed as you left them and restores the focused email in the conversation.
- Pressing <kbd>Tab</kbd> after choosing recipients in **To**, **Cc**, or **Bcc** now moves to the next field, instead of losing your place when contact suggestions were showing.
- Adding a suggested event whose time is unknown now keeps its date and asks you to enter times, instead of selecting **All day**.
- After adding an event from an email to your calendar, press Escape to return to the email and continue processing your inbox.
- Pasting a copied email link into an email address field now removes `mailto:` and link parameters.
- Email images now load when their server sends a malformed content type, including signature dividers that previously appeared as alt text.
- Returning to Mail from Contacts or another workspace now restores your place in a long email.
- Long emails now scroll as one continuous message while composing, keeping the last line readable and the signature directly after your text.

## 0.194.2 — 2026-11-01

### New
- Recipient suggestions now prioritize people you often email together after you select someone. The same suggestions help you add members to contact groups, using email history kept on this device.
- Groups now show **Recent Emails** with their current members, including sent messages. Open a conversation from the sidebar to find other people you may want to add.
- Connect selected Mac address books from **Contacts → Manage Contacts… → Mac Contacts…** to browse people and suggest recipients. Access is read-only, and disconnecting keeps profiles you deliberately saved in ThreeStrands.
- Send from verified Gmail aliases using the composer’s single compact **From** menu, alongside your other accounts. New messages use your Gmail default, replies use the address the message was sent to, and **Settings → Mail Accounts → Sending Addresses** lets you refresh your list after changing it in Gmail. Accounts with only their primary sending address show a compact **Aliases…** action; accounts with aliases show the full list, with default and reply-to details underneath.
- Snooze conversations until later with **h** (Remind Me), then find them in **Snoozed** to reschedule or return them to Inbox. Snoozed mail leaves Inbox in your other mail apps too, and returns when ThreeStrands is running and connected, or when a new reply arrives.
- **Settings → Diagnostics** now shows recent AI rejection reasons, provider and model details, and request references, and lets you open the application logs folder. You can temporarily capture rejected scheduling values for troubleshooting on the affected device.

### Improved
- **Groups → Add Members** now keeps contact names, addresses, and shorter suggestions together in readable rows. Search and actions stay visible while you browse, with clear selection checks and full suggestion details available from the keyboard.
- Contacts remembers your selected person separately in **All Contacts** and **Keep in Touch**, plus your selected group in **Groups**, when you return from Mail.
- Press **b** in Mail or Calendar to create an event, matching Superhuman. Calendar also keeps its existing **Shift+N** shortcut.
- Hidden AI suggestions now say they couldn’t be verified, with an expandable explanation covering invalid scheduling data as well as unmatched evidence.

### Fixed
- New messages use the account whose mailbox you are viewing instead of always falling back to the default account.
- Active mail searches keep their search controls visible, and returning from Contacts or another view restores focus to the search box. A **Close Search** button lets you clear the filter and return to your inbox.
- Meeting suggestions in Brief are less likely to be hidden because of invalid date or timezone formatting.
- Links wrapped by Google or Microsoft Safe Links no longer trigger a link-text mismatch warning when the embedded destination matches the displayed site. Checks for misleading or suspicious destinations still apply, and clicking keeps the original wrapped link.

## 0.188.3 — 2026-10-30

### New
- Events in the weekly calendar now show their location inside the event block. Short meetings keep it inline, while longer meetings show it below the title.

### Improved
- In **Calendar**, it's easier to tell when some accounts are hidden. The header says how many accounts are showing, such as *1 of 3 accounts*, instead of naming them as if you'd switched to one, and the **Calendars** button shows the same count. **Show All** in the calendar list turns each hidden account's primary calendar back on without turning on every other calendar. The schedule beside your mail (**t**) uses the same wording and offers the same **Show All**.
- In a smaller window, Mail gives the open conversation the room. The panel beside it slides over from the right when you need it: use the panel button at the end of the conversation's toolbar, or anything that shows something there, such as getting a brief or pressing **t** for the schedule. Click back into the conversation or press Escape to put it away. The inbox list also narrows with the window instead of keeping the width you set in a larger one.

### Fixed
- Closing an untouched **Reply**, **Reply All**, or **Forward** now removes the draft instead of leaving it in Drafts. Your writing, header changes, and added attachments still keep the draft.
- Meeting invitations no longer list themselves as scheduling conflicts, including recurring meetings and copies on another calendar.
- Picking an account in the sidebar, or with <kbd>⌘/Ctrl + 1</kbd> through <kbd>⌘/Ctrl + 9</kbd> and <kbd>⌘/Ctrl + 0</kbd>, now always takes you to that account's mail, even from Calendar, Today, or another view. Before, it only filtered the view you were in, so in Calendar it seemed to do nothing.
- **Event Details** now gives the **Reschedule** button more space below the event information. The Edit and Delete button tooltips stay inside the right edge of the window.
- A long reply written below a conversation no longer pushes **Send** out of view. As the reply grows, or while Draft Review is open, the conversation scrolls to keep the bottom of the reply showing.

## 0.186.0 — 2026-10-29

### Improved
- The search box in **Files** now also matches who sent a file, by name or email address, so you can find an attachment when you remember the sender but not the filename.

## 0.185.0 — 2026-10-29

### New
- Check off a task in **Open tasks** beside the calendar week to mark it done. It stays a moment, struck through, so you can uncheck it, and **Undo** in the message that follows brings it back. Dragging a task onto the week still books time for it.

## 0.184.0 — 2026-10-29

### New
- IMAP accounts now receive inbox updates as the server announces them, with automatic reconnection and regular checks when live updates are unavailable.

### Improved
- Mail now checks for updates every 30 seconds while the app is active, and gradually checks less often in the background.
- Open calendars and agendas now refresh every 30 seconds and when you return to the app, so changes made in Google Calendar appear automatically. Your events stay visible while refreshing, and automatic checks pause while the app is in the background.

## 0.183.1 — 2026-10-29

### New
- Keep separate hours for different parts of your life, such as work and personal, in **Settings → Availability**, and choose which profile each mail account uses. **Find Times** and checks of offered times then use the hours for the account the email came to; every calendar still counts as busy. To use other hours for one conversation or message, choose them under **Usual Hours** in Find Times.
- When someone offers several meeting times, **Find Times** checks each exact option against your calendars and keeps conflicts, evenings, and weekends visible. Select the free times to draft a reply, change an assumed timezone, or choose **Find Other Times** to search your usual hours.
- Mark code in a message as a **Code Block**: select it and press ⌘⇧0, or type ``` on its own line and press Enter. Code blocks keep their spacing and line breaks and arrive in a fixed-width font. Press Enter on an empty last line to continue writing normally.
- Find **Help and Documentation** in the command palette, or open the relevant guide from Settings. Help now includes search, troubleshooting, and dedicated guides for Today, Files, privacy, and moving between computers.

### Improved
- While text is selected, the composer's shortcut hints now show **Code Block** instead of Underline, plus **Bulleted List** and **Numbered List** when the window is wide enough. Underline is still ⌘U.
- The side panel's **Brief** is easier to scan, with a simpler source line and no empty **Suggested** section repeating the brief. **Find Times** stays available for task suggestions as well as meetings to arrange (or while you reply), including requests to reply with your availability.
- **Recent emails** names the person it's about and marks invitation answers with a calendar icon. Automatic replies are dimmed and listed after the person's own emails.

### Fixed
- Proposed meeting times now use the context panel’s text sizes and aligned selection buttons, with each option numbered once.
- Briefs still load when proposed meeting times cannot be verified, with a **Find Times** retry beside the warning. Harmless spacing and punctuation differences in quoted options no longer reject them.
- The previous-day and next-day tooltips in the Today sidebar now stay inside the app window.
- Forwarding an email no longer shows its saved draft as an extra message in the conversation.

## 0.178.10 — 2026-10-28

### Fixed
- Inbox previews no longer show stray `<br>` tags from emails whose plain-text version contains HTML line breaks.

## 0.178.8 — 2026-10-27

### Improved
- **Draft Review** keeps Send within reach while you read feedback. Applying a revision returns you to the updated message and collapses the review into a card you can expand or undo.
- Inbox previews now show more of what each email says. They stop before quoted replies, including Outlook-style headers; skip lines like **View this post on the web**, divider lines, image labels, and a first line that only repeats the subject; and for a forward with no comment, show the forwarded message instead of your signature.
- Sender warnings stay in the email rather than repeating in the side panel, with familiar-name warnings showing the sending and previously seen addresses on separate labeled lines. Choose **Trust this address for this account** after reviewing its scope, or undo it with **Forget confirmation** in the side panel’s **Sender checks** section.

### Fixed
- Long warning text in the **Be careful** hover card now wraps so you can read the full explanation.
- Sent replies now offer **Reply**, **Reply All**, and **Forward** even when the mail server’s Sent copy has not arrived, and the open conversation keeps checking for delayed Sent copies.
- Drag address badges between **To**, **Cc**, and **Bcc** when composing a message, including dropping onto the field label or input.
- Emails with gradient backgrounds now keep their background colors and patterns, so light text stays readable.
- After you reply to someone, the Brief no longer suggests replying to them or offers to find a meeting time you already sent, and it waits for their next message instead of briefing your own reply.

## 0.177.0 — 2026-10-27

### New
- **Find Times** can include selected coworkers on your email, checking their primary calendars. In **Settings → Calendar Accounts**, choose which of your own calendars affect availability separately from those shown in Calendar; primary calendars are used by default, available holiday calendars are included automatically, and failures identify the affected calendar and account.
- **Calendar** now lists your **Open tasks** under **Today's agenda**: everything in your **To Do** and **In Progress** columns. Drag a task onto the week to book 30 minutes for it, with **Undo** if you change your mind, or click it to pick a time yourself. Booked tasks say when their time is, and opening the event lets you mark the task done.

### Improved
- When an email uses the name of someone you know but comes from a new address, you can now choose **This is legitimate** right in the warning above the email. Confirming that address stops the name warning for its later mail, while other security checks still apply, and the warning beside the sender's name no longer repeats what the box above the email already says.

## 0.174.3 — 2026-10-27

### New
- Choose **Reschedule** on a meeting you organize to see guests’ busy times and suggested alternatives within your working hours. Calendars that cannot be checked are marked as unknown, and guests are notified when you save the new time.
- **Today** now reads your day for you. A line under the date says how many meetings you have, when you're free until, when you're double-booked, and when your last meeting ends. Overlapping meetings are marked on the agenda, with **Recorded** when the invite says the session will be recorded, so you know which one is safer to miss. An unread message from someone you're meeting later today says so, such as "Also in your 2:00 pm meeting".
- **Today** has a new **You said you would** section listing promises from your own sent mail in the last two weeks, like "I'll send the report over", when you haven't written to that person since. Click one to open the conversation, turn it into a task, or dismiss it.
- The reading pane now shows a quiet shield for familiar, verified senders or a warning that explains what needs a look, with details on hover or click and the strongest warnings above the email. In the side panel’s **Be careful** or **Sender checks** section, choose **This is legitimate** to remember a requester’s sending address and link destinations on this device, with **Forget confirmation** to undo it; matching authenticated requests get a quieter assessment while delivery changes and other security concerns still warn.
- **Draft Review** can now know who you are without being told every time. In the new **Settings → Writing Profile**, describe yourself in **About You** and say how you want drafts reviewed in **Review Instructions**, separately for each mail account, with room for a few paragraphs in each. Clicking **Review** now goes straight to feedback, with no preview and second click. Add a goal and click **Review Again** when you want it to aim at something specific, and open **What was sent** to see exactly what went to your AI provider, including your profile. Draft Review suggests using a fact about you only as a suggestion you can see. Profiles sync to your other devices and are included in settings exports.
- Reopen a long email you were partway through, such as a newsletter, and it picks up where you left off instead of starting back at the top. Your place is kept for two days, and the email opens at the top again once you've read to the end or a new reply arrives.

### Improved
- **This is legitimate** confirmations now sync between your devices for the same mail account, including confirmations you already saved. **Forget confirmation** also syncs, while security checks continue to apply.
- **Today** is easier to read at a glance. Event, task, and message titles are a size larger, the lines under them match your inbox's titles, and the header is down to the date and one line about your day, since unread mail and tasks due already have their own columns.
- Removed the **History with** person selector from the right context panel. Recent emails and files still follow the message you’re reading or the recipients you’re writing to.
- Calendar invitations in the side panel's **Files** list are now named by their meeting, as they are in **Files**, instead of all showing as "invite.ics".
- A calendar event's details in the week view's side panel are tidier. The close, edit, and delete buttons stay beside the event's title, the attendees and description lines match the rows above them, a location that's only a link shows its site name, and related files and conversations follow right after the details instead of below a large gap. **Going?** now sits beside Yes, No, and Maybe instead of a separate response line, and the calendar line shows the calendar's name, or is left out when it would only repeat your one account.
- For a recurring meeting, the side panel's **Files** list shows that meeting's invitation instead of one from every past occurrence, and a reason that applies to every file or conversation, such as "Matching topic and attendees", appears once under the heading instead of on each row.

### Fixed
- Emails with no visible recipients, such as Bcc-only messages, no longer show an empty “to” line.
- **Draft Review** now shows a sparkle icon beside its heading instead of an empty gap.
- When an IMAP mail server's security certificate changes after you trusted it, the account now pauses and asks you to review the new certificate in **Settings → Mail Accounts**, instead of retrying with a "Could not reach the server" message or asking for your password. Messages waiting to send say the same and go out once you trust the new certificate.
- Moving the start date or time in **Edit event** now moves the end along with it, keeping the event’s duration. Changing the end date or time sets the duration for subsequent moves, including all-day events.
- Pressing **Escape** after opening a calendar event from **Today** now returns you to **Today**, just as it does after opening an email.

## 0.167.1 — 2026-10-25

### New
- Email text that's hard to read, such as dark blue on a dark background or pale gray in a signature, is now lightened or darkened just enough to read while keeping its color. Choose **Use App Colors** under **Settings → Appearance → Email Text Colors** to replace sender colors with your theme's, or **Keep Sender Colors** to turn this off.
- On Windows and Linux, ThreeStrands can now open email links and `.ics` calendar invitations. Set it up in **Settings → Default Apps**: Linux makes the change right away, and on Windows the button opens Windows Settings, where you choose ThreeStrands. A link or invitation opened while ThreeStrands is running goes to the open window instead of starting a second copy.
- Press **⌘D** (**Ctrl+D** on Windows and Linux) to switch between light and dark mode from anywhere in the app, including while writing a message.
- When you create an event, **Invite People** now suggests your contacts and contact groups as you type a name or address. Press **Enter** or click a suggestion to add someone as a badge with their name, or choose a group to invite everyone in it.

### Improved
- The conversation sidebar is calmer: single-takeaway briefs read as paragraphs, contact history has a compact person selector, known-sender details fold away, and upcoming meetings include their times. Respond to invitations and suggest another time directly on the invitation card, with your current response clearly selected, a single meeting link, and expandable event details.
- On Windows, **Files** now shows a preview picture of PDFs, images, and other documents, as it already did on macOS.
- Text that isn't an email address no longer turns into a recipient badge when you write a message or invite people to an event. It stays in the field with a note saying what's wrong so you can fix it, and **Send**, **Send later**, and creating the event wait until it's fixed.

### Fixed
- Opening a long conversation now shows the newest message at the top of the reading pane, instead of leaving it partly hidden at the bottom while its content loads.
- Meeting suggestions now prefer the full conferencing join link over a generic location such as “Microsoft Teams.” Emails offering multiple sessions keep each session paired with its own join link.

## 0.163.2 — 2026-10-24

### New
- **Settings → Appearance → Email Zoom** now enlarges email text, images, and spacing together, independently of app text size. The reading controls explain how to combine zoom with a minimum font size, or preserve the sender’s proportions by leaving the minimum off.
- The new **Today** view (press <kbd>0</kbd>) shows your day at a glance: your agenda with a **Join** button for the next meeting, unread mail sorted into **Invitations**, **Needs a reply**, **From people**, and **Updates**, and tasks that are overdue or due today. Finished events are dimmed under a **Now** line, and the first few events **Tomorrow** are listed below. The agenda includes all your calendars, while mail and tasks follow the account you have selected. Its lists use the same text size as your inbox, with room to breathe. To start the app there, choose **Today** under **Settings → Appearance → Start With**; when your inbox is empty, the side panel links to it.
- You can now edit the message you're forwarding. Click **Edit** on the bar above the forwarded message to trim it or add notes. Some formatting and web images may be simplified, and **Restore Original** brings back the message as you received it.
- Calendar invitations now offer separate **Accept**, **Maybe**, and **Decline** buttons, plus **Suggest a different time**, in the side panel, replacing duplicate AI event suggestions; choosing alternative times prepares a reply for you to review. Other events suggested in the **Brief** recognize when they’re already on your calendar, even at a different time or under a new name, with **Open in Calendar** to see them, **Dismiss** to clear the suggestion, and **Add Again** in **⋯** if you need another copy.
- The header of an open email now shows who was copied, as in "to David Martinez, cc me", so you can tell whether a message was sent to you or you were only Cc'd. Your own address shows as "me".
- Meeting details in the **Calendar** now show who has accepted or declined. The guest count adds a tally such as "2 going, 1 not going", and each guest is marked **Going**, **Not going**, **Maybe**, or **Awaiting**.
- ThreeStrands now reopens where you left off. If you quit while in **Today**, **Calendar**, **Tasks**, **Contacts**, or **Files**, that view is open the next time you start the app.
- **Settings → AI Provider** now lists recent **Activity**: which feature sent each request to your AI provider, the conversation's subject, how many messages, and the tokens used. Email text is never kept there.

### Improved
- Switching accounts with <kbd>⌘1</kbd>, <kbd>⌘2</kbd>, and so on now remembers the email you had open in each account, so it's still selected when you come back.
- Pressing <kbd>t</kbd> now switches the column on the right of your mail between the conversation's details and the day's **Schedule**, instead of squeezing in another column. Your choice stays put when you visit another view or restart the app.
- Calendar invitation cards now make web addresses in the location clickable, opening them in your browser, and keep decorative separator lines out of descriptions. In the side panel, unanswered invitations now say **Not responded.**, and conflicting meeting titles appear in quotation marks.
- The **Brief** now sums up what's new since your last reply or your last brief, instead of the whole conversation again, and suggests tasks only from the new messages. When it helps, a last line recaps the earlier conversation, and a note says when the brief covers only the latest messages.
- Meeting suggestions in the **Brief** are easier to act on. Ways to narrow the times (**Any Time**, **Mornings**, **Afternoons**) and the arrows for earlier or later days sit above the times, picked times show a checkmark, and **Draft Reply With These Times** stands out as the main button. **Edit** and **Dismiss** move to the card's **⋯** menu, and **Check details** now opens the editor.
- Meetings suggested in the **Brief** show where and who with small icons, and list names in reading order, such as "Joshua Dadeboe" rather than "Dadeboe, Joshua". Online meetings get a **Join** link when the email or your calendar has the meeting address. **Discard** on a suggestion is now **Dismiss**, and the email excerpt sits at the bottom of the card.
- The **Brief** section collapses from its heading like the rest of the side panel, and its bullets and text now line up with the section headings. It also says how many messages were shared with AI and with which provider.
- Discarding drafts, or failed messages in **Outbox**, now works like moving conversations to Trash: it happens right away, and **Undo** in the notice (or <kbd>z</kbd>) brings them back, attachments included, for up to an hour. Failed messages come back to **Drafts**.
- Collapsed emails in a conversation now use the same text sizes as the rest of the app, instead of larger type.
- An unsent draft at the end of a conversation now lines up with the emails above it, with **Resume Draft** on the right.
- You can now reorder accounts in **Settings → Calendar Accounts** with the up and down arrows, the same way as mail accounts. The **Calendar** sidebar follows the new order, and reconnecting an account no longer moves it to the bottom of the list.
- Files you attach to a message now appear right above **Send** and the paperclip button, so you can see them as soon as you add them instead of having to scroll down past your signature.
- When suggested meeting times can't be checked against one of your calendars, a single note under the times now names that calendar and links to **Calendar Settings**, instead of "not every calendar checked" on every time. Times for an email that doesn't give a time zone now say which zone they use in familiar terms, such as "Eastern Time".

### Fixed
- The agenda in **Calendar** no longer lists a meeting twice when it is on two of your calendars; it shows the copy you haven't declined. Declined meetings are dimmed, and calendars named after your account no longer repeat the address.
- **Settings → Appearance** text size, font, and minimum email font size now stay on each Mac instead of syncing to your other devices, so you can use larger text on a laptop without changing it on a big monitor.
- Emails that set their text size, color, or weight with older-style formatting now look the way the sender designed them, instead of showing headline figures as small bold text. With a minimum font size set in **Settings**, footnote numbers no longer push their line apart.
- Gmail drafts now stay in step for accounts with no drafts. Before, if you sent or deleted your last draft in Gmail or on another device, it could stay behind in **Drafts** here.
- A message you forward now shows in its conversation as soon as you click **Send**, the way replies do, instead of appearing only after it finishes sending.
- Clicking a conversation in the message list while writing a new message now opens it. Your message is saved to **Drafts** so you can finish it later.
- Newsletters that space out their sections with blank table rows, such as article cards with a gap between the headline, image, and summary, now keep that spacing instead of running the sections together.
- Full-size photos taken on recent phones now load in emails when remote images are on, instead of showing as empty boxes.
- Images in emails from senders whose image servers screen out non-browser visitors now load when remote images are on, instead of staying blank.
- The **Brief** for an email you're only copied on no longer treats requests made of someone else as yours, and doesn't suggest them as tasks for you. Briefs are also shorter, so a short email gets a single line.
- With **Drive Time** on, suggested meeting times no longer warn that a calendar wasn't checked when only its travel time couldn't be read. Its busy times are still checked.

## 0.151.3 — 2026-10-21

### Improved
- **Drafts** now looks like your other folders, showing who each draft is to, its subject, and a preview. You can select several drafts with ⌘-click, Shift-click, or ⌘A and discard them together, after confirming.
- In **Outbox**, you can now select several messages the same way: return the ones still waiting to send to **Drafts**, or discard the ones that failed. Its rows also line up with every other folder.
- Newsletters, receipts, and notifications now preview in the message list with the summary line their sender wrote for inbox previews, like "Your login code is 564368" or "Delivered 1 item", instead of a logo name or a "View in browser" link.

### Fixed
- Shift-clicking a checkbox in a folder list now selects every message between it and the last one you checked, as Shift-clicking a row already did.
- Emails that center their layout with older formatting, common in newsletters and promotions, now appear centered in the reading pane instead of pinned to the left edge.
- Some newsletters and promotions previewed in the message list as placeholder codes like `${Headline-Text}`, or as a note that your email software can't display them, instead of their words. The preview, and searches, now use the text the email actually shows.

## 0.150.1 — 2026-10-20

### New
- Drafts on Gmail accounts now save to your Gmail **Drafts**, so you can pick them up in Gmail on your phone or the web. ThreeStrands saves a few seconds after you stop typing, and right away when you close the message. The composer says **Draft saved** once Gmail has it, or tells you when it's waiting to reconnect. Sending or discarding the draft removes it from Gmail.
- Drafts you start or change in Gmail, on any device, now appear in **Drafts** within a minute, attachments included. A reply you started elsewhere shows at the end of its conversation with **Resume Draft**, and pressing **Reply** opens it. If a draft changes both here and in another app, the composer asks which version to keep.
- Drafts now reach your other computers through Gmail instead of **Replicated Sync**, so attached files come along. A signature added on another computer arrives as part of the message text.
- Drafts on IMAP accounts, including Proton Mail Bridge, now save to the account's Drafts mailbox and come back from it, just like Gmail drafts, so you can finish them in another mail app. If you didn't map a Drafts mailbox during setup, drafts stay on this computer.

### Fixed
- The composer now says **Draft saved** instead of "Saved on this device" once a draft is safely stored, including while it waits to sync or needs your attention, and the **Drafts** list no longer says "Saved on this device" either.
- After you reply to a conversation, your sent message now shows **Reply**, **Reply all**, and **Forward** within a few seconds, so you can reply to everyone again without reopening the conversation.

## 0.147.0 — 2026-10-20

### New
- Drafts now follow you between your computers with **Replicated Sync**: start a message on one and finish it on another. Sending or discarding a draft removes it everywhere. Attached files stay on the computer where you added them, so attach them again before sending from another one. If the draft you have open changes on another computer, the composer switches to that version, or asks which to keep when you have unsaved edits.

### Improved
- **Settings → AI Provider** is simpler. Once you choose a provider, AI appears in reading, writing, and contacts, and it still sends a conversation only when you click a button marked with sparkles. Under **Show AI In**, switch off any area you don't want. **Automatic Briefs** is now one choice: **Never**, **People I've Emailed**, or **All Conversations**. Model choices and usage are under **Models and Usage**.

### Fixed
- **Remove on all devices** now also cancels messages waiting to send from that account on your other devices, so adding the account back later never sends them unexpectedly. They stay in the **Outbox** marked as failed, where you can restore them.
- **Remove on all devices** now removes an account from your other devices even when its address was typed with capital letters.
- Pictures in some newsletters and announcements now appear when **Load remote images** is on. Previously they stayed blank if the sender's server didn't label them as images.
- Event details in the calendar side panel now show attendees and the description in the same text size as the rest of the panel, and a single guest reads "1 attendee".
- Calendars on your other accounts keep loading when a calendar account you connected on another device syncs to this one. Previously **Settings → Calendar Accounts** said every account's calendars couldn't be loaded until you disconnected that account here. A newly synced account now also appears there without restarting.
- Inbox previews for some newsletters no longer show a wide blank gap followed by stray text like `&zwnj`. Conversations already in your inbox are fixed automatically.
- Escape now closes search after you open a result. Previously it worked only while the search box had focus.

## 0.145.2 — 2026-10-19

### New
- The search box now suggests as you type, the way Gmail does: type `from:` or `to:` to pick someone from your contacts, `label:` or `in:` to pick a label or folder, or `after:` to pick a date like **Past month**. An empty search box lists your recent searches. Choosing someone with several email addresses searches all of them.
- The **Trust** section now says **Be careful** when a subject line disguises its words with look-alike characters, such as a lowercase l in place of a capital I ("REVlEW"), a zero in place of an O, a letter from another alphabet, or an invisible character, and it names each disguised word.
- Use keyboard shortcuts to attach files, refresh mail, create calendar events and contacts, save contacts, and open, save, or star files. **Keyboard Shortcuts** now also lists snippets, recipient-field focus, and calendar navigation.

### Improved
- In **Settings → AI Provider**, the **Reasoning Model** and **Fast Model** fields now show a searchable list of the models your provider offers, so you can type to filter and pick one instead of remembering the exact name. You can still type a model name the list doesn't include. After you switch providers, **Refresh model list** loads the new provider's models with your saved key.
- Search now looks through all your mail, including archived conversations, without clicking **Search All Mail** first. Results still in your inbox are marked **Inbox**, **Inbox Only** narrows a search to your inbox, and Trash and Spam stay out unless you search with `in:trash` or `in:spam`. The server is searched automatically once you stop typing.
- Search understands a minus to leave things out (`invoice -from:billing@example.com`) and `OR` or braces for either-or (`from:ana OR from:bo`), in the results on this computer as well as on the server.
- Mail shortcuts now act on checked conversations together, and Select All selects the conversations visible through your filters. Undo also handles pending sends and task completion, and search stays in **Files** or **Contacts** when you’re working there.
- **Settings → Signatures** now lists your default signatures first, one compact row per address with its email, and shows any alias that uses its own signature right under its account. Each signature shows its first few lines as written and which addresses use it, and deleting one tells you which addresses will lose it.
- New installations keep mail locally for 90 days by default; existing retention settings stay as they are. You can change this in **Settings → Privacy → Keep Mail on This Device For**.

### Fixed
- In replies, the trash icon now lines up with **Review** above it.
- When one part of the window runs into a problem, only that part shows **Something went wrong** with **Try Again**, and the rest of the window keeps working instead of going blank. Choosing another conversation or mailbox also brings it back.
- Up and down arrow keys keep switching Settings sections after you click a section with the mouse.
- Keyboard navigation in the mail list now clears the gray hover highlight until you move or click the pointer, keeping the selected conversation clear.
- Searching for `to:` someone now also finds mail where they were on Cc or Bcc, as it does in Gmail.
- Trashed conversations now follow your local mail retention setting, even when starred. Other starred conversations stay on your device, and local cleanup leaves mail on the server.
- With **Load remote images** on, images now load in emails from senders whose image servers turn away anything that doesn't look like a web browser. Images are still fetched through ThreeStrands, so senders still don't see your browser, cookies, or which message you're reading from.
- Newsletters and other emails that center their content in a column now appear centered, as they do in Gmail, instead of hugging the left edge of the reading pane.

## 0.141.0 — 2026-10-16

### New
- Set a **Drive Time Buffer** at the bottom of **Settings → Availability** to reserve time before and after calendar events with a location. Meeting suggestions and conflict checks leave that time free for travel.
- In **Settings → Signatures**, **Import from Gmail…** brings in a signature from a connected Gmail account for preview, naming, and optional use as the account’s default for new messages. You can edit its plain-text alternative separately or regenerate it from the visual signature.
- The **Trust** section in the side panel now spots common email tricks using only what's on your computer: a familiar name sent from a new address, an address or link that imitates a domain you write to, links that hide where they go, and risky attachments from people you haven't written to. It says **Be careful** when something looks wrong, shows **Known sender** for people you write to, and **Show link** scrolls to the link in question. Sender checks, linked sites, and tracking images are under **Details**. **Known sender** also appears when a forwarding or filtering service left out the usual sender check, as long as the email is confirmed to come from the sender's own domain.
- When your inbox is empty, the side panel now shows what's next: your open tasks, people you're due to reconnect with from **Keep in Touch**, and today's agenda when you have a calendar connected. Click any item to open it.

### Improved
- The side panel now shows which person its history is about, with one name at the top of that section and a **2 others** button to switch to anyone else on the conversation. Your own colleagues are listed last. As you scroll or move between messages, the panel follows whoever wrote the message you're reading.
- **Find Times** and **Get Brief** now sit together as one row of buttons at the top of the side panel, and each opens below when it has something to show. **Find Times** works while you're reading a conversation, too: pick the times you want and **Draft Reply With These Times** starts your reply.
- **Upcoming meetings** in the side panel show short dates like **Today**, **Tomorrow**, or **Fri**, so long meeting names wrap onto a second line instead of being cut off. Hover a date for the full day.
- The side panel no longer lists other people at the same company. Grouping by email domain mixed in unrelated people at large companies and shared mail providers.
- **Draft Review** now opens below your message from **Review** in reply and new-message windows. In replies, **Draft Reply** and **Review** sit beside the quoted-conversation toggle, with a single divider above **Send**.
- Replies now hide the signature picker when no signature is applied. Your signature’s hosted images appear automatically while composing, without image removal or resize controls over the logo.
- **Settings → Signatures** now uses a visual editor with a compact formatting toolbar, font and color pickers, and image editing when you click a logo. **Import…** is under **More**, and your imported layouts, hosted images, and separate plain-text alternative are preserved.
- Messages sent to a long list of people now show the first 20 recipients followed by "and N more". Click it to see everyone.
- The **Task details** heading in the **Tasks** side panel and the **Brief** heading in the conversation side panel now have icons, matching the other side-panel headings. AI buttons now show their sparkles in the same color everywhere.
- Reply and new-message windows give more room to writing, with a compact signature picker below the message and **Send later…** in the **Send** menu. Replies also offer **Send & archive** in that menu and a shorter **Draft Reply** button on the right; keyboard hints now match your platform.
- When your inbox is empty, the reading pane now says **Inbox zero** and shows when that account was last checked, or that it needs reconnecting or couldn't be checked, so an empty inbox is never mistaken for a stalled sync. Problems with your other accounts no longer show up there. The inbox header is tidier too: **Main** appears only once you have a split inbox, and Add Split and **Filters** are now compact icon buttons beside the conversation count. When filters hide every conversation, **Clear Filters** brings them back.
- Pasting or importing a signature now keeps hosted HTTPS image URLs by default, including images too large to embed. **Embed Remote Images** is tucked under **Image Options** in the Gmail import dialog when you want to send small images with the message instead.

### Fixed
- The **Trust** section in the side panel no longer warns about links in the quoted earlier emails that are folded away in a reply. Those links are often rewritten by someone else's mail system, so their text and destination differ without anything being wrong.
- In **Settings → Signatures**, dialogs now scroll so you can reach the plain-text field and import controls, previews show logos on a white default background, and **Load Images** lets you see hosted logos while editing. Editing plain text during Gmail import now responds without reprocessing the whole signature, formatted phone numbers are no longer repeated, and the **Choose File** button and its label are centered in the file field. Divider lines in a signature, such as a vertical line beside a logo or a rule above a tagline, now stay in the preview and in the messages you send.

## 0.133.2 — 2026-10-15

### New
- Create reusable email signatures with formatted text, links, and logos in **Settings → Signatures**, and choose defaults for each sending address. Switch or remove signatures while composing, with separate defaults for new messages and replies or forwards.
- A **Sent** folder lists the conversations you've sent messages in, including ones sent from your other devices or from webmail. Choose it from the folder menu or press G then S.
- In **Contacts → Groups**, **Email Group** starts a new message addressed to everyone in the group.
- **Edit as New** starts a fresh message from one you sent before, with its subject, text, and attachments but no recipients, so you can send the same note to new people. Use the button beside **Forward** on any message you sent, or press Shift+C. It works in the new **Sent** folder too.

### Improved
- The **Outbox** now holds only mail that hasn't finished sending: messages you can still undo, scheduled sends, and ones that failed or need a check. Once a message goes out it moves to **Sent**. The Outbox also works like your other folders now: choose a message on the left to read it on the right, with its delivery status and actions such as **Undo Send** and **Reschedule** above it.
- For newsletters and other bulk mail, the side panel now lists the sender's earlier emails in **Recent emails** instead of a separate card, with **Unsubscribe**, **Mute…**, and **New Split…** as small links at the bottom so they don't compete with the email you're reading.
- Buttons that sit side by side, such as **Add Members** and the delete button on a contact group, now line up with each other.
- When adding members to a group, the people you've chosen now appear inside the search field, the search clears after each pick, and Backspace removes the last person.
- A message you **Forward** now stays in the conversation it came from, so you can see what you sent on and read your coworkers' replies alongside the original thread.
- Finding times to meet now happens right in the side panel. With **Draft Assist** turned on in **Settings → AI Provider**, **Find Times** reads the conversation and your draft, so when someone asks to connect next week you get times next week, with a line saying why. Type a change such as "Thursday or Friday afternoon" and click **Adjust**, or narrow times to **Mornings** or **Afternoons**, move to **Later** dates, or click **Show More**, all without a second calendar column squeezing the email you're reading. If a brief already suggested a meeting, Find Times starts from the dates it found. Asking "When am I free Thursday afternoon?" in **Ask About This Conversation** now shows only Thursday afternoons. The **Find a time** section of the day calendar has been retired in favor of this.
- In **Files**, the **File details** heading now has a file icon that lines up with the other icons in the side panel.
- Buttons that send something to your AI provider now all carry the sparkle icon, so you can tell at a glance when AI is involved. AI features you switch off no longer leave reminders in the side panels or the command palette; until you choose an AI provider, the **Brief** section offers a single **Set Up AI** link. The command palette now calls the brief **Get Brief**, as the button does.

### Fixed
- Tables pasted from a spreadsheet, such as a price list, no longer show up as a tall blank box with nothing inside.
- Adding members to a group no longer offers to create a new contact for an address that already belongs to someone in your address book; it picks that person instead. Typing the address of someone already in the group now says so rather than adding them again.
- The folder menu (Inbox, Drafts, Outbox, and the rest) works again while a draft is open, and switching folders saves what you've typed first.
- In **Calendar**, the button that hides the side panel no longer covers the event count next to **Today's agenda**.
- In **Files**, the sender in **File details** no longer has "Opens in Contacts" stuck to the end of their name.
- **Draft Reply With AI** and **Enhance With AI** now say what setup is missing when no API key is configured, instead of doing nothing or failing after you click.

## 0.125.4 — 2026-10-15

### Improved
- Removing a Gmail account or Google Calendar now also signs ThreeStrands out at Google, so the access it had stops working instead of staying active until you revoke it yourself. If ThreeStrands can't reach Google, the account stays put so you can try again. Removing only the mail or only the calendar keeps you signed in while the other is still connected on this computer, and if you sync between devices, **Disconnect this device** leaves your other devices signed in.

### Fixed
- Buttons in some emails, such as security alerts, no longer lose their padding and show their label squeezed against the top edge.
- Some emails no longer show small stray gaps between images, buttons, or blocks the sender placed tightly together.
- In **Settings → Mail Accounts**, the Gmail or IMAP badge under **On Your Other Devices** now lines up with the **Set Up Here** button.

## 0.125.1 — 2026-10-14

### New
- You can now set more than one block of working hours on a day in **Settings → Availability**, for example 9:00–12:00 and 13:00–17:00 to keep your lunch hour free. Suggested meeting times leave the gaps alone.
- Image attachments on a message now show a small preview of the picture, so you can tell them apart without opening each one.
- When an email announces events, such as a school or club calendar, **Get Brief** and **Ask about this conversation** now offer each one as an event you can add to your calendar at its stated time. Events with a date but no time yet are added as all-day events.
- New events can now be marked **All day** and given a location.
- Newsletters and other bulk mail now show who sent them in the side panel: how many emails they've sent, how often, and how many you've read, with one-click **Unsubscribe**, **Archive** for everything still in your Inbox from them, and **New Split…** to give them their own tab. **Mute…** goes further: new mail from that sender is marked read and archived as it arrives, on all your devices, and you can review muted senders in **Settings → Muted Senders**. If you sync between devices, update ThreeStrands on each of them to share muted senders.
- A new **Trust** section shows whether your mail server verified the sender, which sites the links open, and any tracking images. It also appears on any email where a link's text names a different site than it opens, or replies would go to another organization.
- Attachments that could harm your computer, such as programs, Office files with macros, disk images, attached web pages, and files disguised as documents like invoice.pdf.exe, now show a warning and the reason in the message and in **Files**, before you open them.
- **Files** can now mark a file's email as spam with **Mark Spam**, which removes its files from **Files**. **Undo** brings them back.
- On macOS, **Files** now shows a preview of the selected file, such as the first page of a PDF or document, above its details. Click the preview to open the file. Files that could harm your computer are never previewed.

### Improved
- Times you offer or confirm in a reply now end with a familiar time zone abbreviation such as EDT or PST, instead of a name like America/New_York.
- Attachments on a message now say what each part does: click the filename to open it, use the save button to save a copy, and the star at the end keeps it in **Files**. Attachments stacked in a column now line up.
- Suggestions now read up to 10,000 characters of each message, so events further down a long newsletter aren't missed.
- In **Files**, the details panel now lines up with its heading, and the **Star** button has a label like **Open** and **Save as…** beside it. Hover over the sender's name to see their contact card, or click it to open them in **Contacts**. **Open** is now the main button, and **Source conversation** shows just the email's subject instead of repeating the sender and date.

### Fixed
- Returning to **Files** no longer briefly shows files from emails you marked as spam or deleted while you were away.
- Emails written as preformatted text, such as some security and system notices, now wrap long lines to fit the reading pane instead of cutting them off at the edge.
- Small icons in emails that specify only an image height now keep their proportions instead of appearing squeezed beside text.
- Hovering over a message no longer shows a stray "Message content" tooltip.
- An email that lists more than 10 events or to-dos no longer makes suggestions fail. The first 10 are shown, along with a note saying how many more there were.
- Suggested meeting cards no longer flash "Checking your calendar…" from time to time while you read.
- Adding an announced event to your calendar no longer invites the person who sent the email.
- The notice that an account is set up on another device is now a small message in the bottom-left corner, alongside app update notices, instead of a panel filling the window.
- In the side panel's **Files** list, the star no longer covers the date. It now sits after the show-email button, under the date.
- The reply, reply all, and forward buttons beside a sender's name now line up with the name instead of sitting slightly below it.

## 0.116.1 — 2026-10-14

### New
- Accounts you set up on another computer now appear in **Settings → Mail Accounts** under **On Your Other Devices**, instead of showing as broken. Choose **Set Up Here** for the ones you want on this computer, so a personal account doesn't have to follow you to a work laptop.
- Importing an encrypted settings file lets you choose which of its mail accounts to set up on this computer.

### Fixed
- Background images in messages, such as social icons and banner panels, now keep the size and position the sender gave them instead of appearing cropped.
- After installing an update on macOS, ThreeStrands now reopens in front of your other windows instead of behind them.

## Highlights: September to October 2026

ThreeStrands went from its first line of code to its first public releases in about a month. These are the things it can do today.

### New
- **Gmail and IMAP in one inbox.** Connect several Gmail accounts and any IMAP/SMTP server, including Proton Mail through Proton Mail Bridge. Read them together in one inbox or focus on one account at a time.
- **Keyboard-first triage.** Archive, star, label, and undo without leaving the keyboard. Split Inboxes divide your mail into tabs, and the command palette (⌘/Ctrl + K) finds every action by name.
- **Search that understands you.** Narrow searches with `from:`, `to:`, `subject:`, dates, labels, and attachments across every account. **Search All Mail** also asks the server for older mail.
- **Writing tools.** Rich text, snippets, attachments, Send Later, a 10-second undo window, and checks before a message goes out.
- **Tasks and goals.** Turn a conversation into a follow-up and roll your follow-ups up to goals for each account.
- **Calendar.** Connect Google Calendar, plan your week in Day, 3-day, Workweek, or Week view, create events, and share your free times. On macOS, ThreeStrands can open email links and `.ics` invitations.
- **Contacts.** Profiles, groups you can address in one step, birthdays, Keep in Touch reminders, and CSV and vCard import and export.
- **Files.** Every attachment from your mail in one place, with filters for sender, date, type, and account. Star the ones you'll need again.
- **Optional AI assistance.** Bring your own provider key for conversation summaries, thread chat, suggested tasks and meetings, and reply drafts. Nothing is sent until you turn it on.
- **Settings transfer and sync.** Move your setup to a new computer with an encrypted export, or keep several computers in step with end-to-end encrypted sync.
- **A Getting Started checklist** that checks itself off as you connect mail and try the essentials.
- **One-step unsubscribe** from mailing lists that support it, with a confirmation first.

### Improved
- **Mail looks the way the sender designed it.** Messages render faithfully inside a locked-down sandbox, remote images stay behind a proxy until you allow them, and long quoted replies fold out of the way.
- **A calmer, more consistent interface.** Buttons, type, spacing, and icons now follow one design system across mail, tasks, calendar, and settings.
