---
title: Write Notes
nav: Notes
summary: Keep a note for each day and notes of your own, written in Markdown, linked to your mail, meetings, and contacts, and synced between your devices.
reviewed: 0.210.0
---

## Open Notes {#open-notes}

Click **Notes** in the sidebar or press <kbd>4</kbd>. The list on the left starts with **Today**, then the notes you opened or changed most recently on this device. Notes reopens the note you had open last.

Press <kbd>j</kbd> and <kbd>k</kbd>, or <kbd>↓</kbd> and <kbd>↑</kbd>, to switch notes, even when the list doesn't have focus. While you're writing, the arrow keys move the cursor instead. Drag the right edge of the list to make it wider or narrower. The side panel on the right resizes from its left edge; drag it past that edge or press <kbd>\\</kbd> to hide it, and press <kbd>\\</kbd> again to bring it back.

## Daily Notes {#daily-notes}

Every day has its own note. The calendar at the top of the side panel opens any day's note, and a dot marks each day you've written in. Its arrows page through the months without opening anything, and **Jump to Today** returns to today.

On a daily note, use the arrows beside the date, or press <kbd>-</kbd> and <kbd>=</kbd> as in **Calendar**, to step to the previous or next day. Press <kbd>t</kbd> to open today's note. The side panel also lists that day's events from your calendars; **Add Note** on one adds the meeting to the note you're reading (see [Meeting Notes](#meeting-notes)).

A day's note is created when you first write in it, so looking at a day leaves nothing behind.

## Write {#write}

Select a note and press <kbd>Enter</kbd>, or click in it, to write. Press <kbd>Esc</kbd> to go back to the list. While reading, <kbd>Space</kbd> and <kbd>Shift</kbd>+<kbd>Space</kbd> page through the open note, as they do for a message.

Notes are written in Markdown and formatted as you type: `**bold**` turns bold, `# ` starts a heading, and `- ` starts a list. Links open in your browser. Notes never load pictures or previews from the internet.

The Markdown formats use the same keyboard shortcuts as email:

| Format | Shortcut |
| --- | --- |
| Bold or italics | <kbd>⌘/Ctrl + b</kbd>, <kbd>⌘/Ctrl + i</kbd> |
| Link | <kbd>⌘/Ctrl + k</kbd> |
| Strikethrough | <kbd>⌘/Ctrl + Shift + x</kbd> |
| Fixed-width | <kbd>⌘/Ctrl + Shift + m</kbd> |
| Numbered or bulleted list | <kbd>⌘/Ctrl + Shift + 7</kbd>, <kbd>⌘/Ctrl + Shift + 8</kbd> |
| Quote | <kbd>⌘/Ctrl + Shift + 9</kbd> |
| Code block | <kbd>⌘/Ctrl + Shift + 0</kbd> |
| Indent or outdent a list item | <kbd>Tab</kbd>, <kbd>Shift + Tab</kbd>, or <kbd>⌘/Ctrl + ]</kbd>, <kbd>⌘/Ctrl + [</kbd> |

Select text to format it. Press the same formatting shortcut again to remove the format. Notes stores plain Markdown, so email's underline and text color formats are unavailable.

Use `/Checklist` for a square checklist or `/Task` for a round task. Round tasks also appear in **Tasks**, and checking one off in either place checks it off in both. Square checklists stay in their note.

Paste or drag a picture or file into a note to add it. Pictures show in the note, and other files show as a small label with their size; click either to open it in its usual app. **Files** lists them too, beside your mail attachments. A note can hold files up to 20 MB each. A file stays with its note until you delete the note from Trash, even if you remove it from the text.

To make a note of your own, press <kbd>Shift</kbd>+<kbd>N</kbd> or click **New Note** and give it a title. Each title can be used once.

ThreeStrands saves a note a moment after you stop typing, and again when you leave it or close the app. Text you haven't saved yet stays on this device, so it's still there if the app closes unexpectedly.

## Link Notes {#link-notes}

Type `[[` to link to another note: pick one from the list, or keep typing a title that doesn't exist yet. `[[Title|other words]]` shows the other words but still links to Title, and `[[2026-11-09]]` links to that day's note. Click a link to open its note; a link to a note that doesn't exist yet creates it.

When you rename a note, ThreeStrands changes the links to it in your other notes as soon as you leave the title. A note in Trash keeps its old links. So does a note that would become too long to sync, or one that was changed on two devices and is waiting for you to choose a version; ThreeStrands names those notes when it renames.

## Link Mail, Contacts, Tasks, and Events {#link-mail-contacts-tasks-and-events}

Type `@` at the start of a word to find a contact or an email conversation by name, address, subject, or participant, and link it. A contact link opens the contact's record, and a conversation link opens the email. An email address you type stays plain text.

To link a task or a calendar event too, click the **+** beside **Links** in the side panel, or **Link contact or conversation** when the note has no links yet. **Link to…** lets you choose a contact, conversation, task, or event and search for it.

While reading a conversation in **Mail**, choose **Add to Today's Note** from the conversation's **⋯** menu to add it to today's note. A conversation, task, event, or contact that a note links to lists that note under **Notes** in its side panel, and a contact's details list the notes that mention them.

## Meeting Notes {#meeting-notes}

Click **Add Note** on a calendar event in **Calendar** or **Today**, or on the day's events beside a daily note, to write a line about the meeting under **Meetings** in that day's note, such as "3:00 pm met with Dana for Monthly Review". The line links the event and each attendee's contact. Before adding it, you can change the **Meeting name** and the **Attendees**; your own addresses are left out.

Check **Create backlinked note** to also link a note named after the meeting. Every later meeting with the same name links the same note, so the note collects that meeting's history. ThreeStrands says whether it will link an existing note or create a new one. Click **Add to Daily Note** to add the line; the message that follows offers **Open Note** and **Undo**.

When a meeting already has a note, its details offer **Open Note** instead of **Add Note**, and the calendar's note actions open that note rather than adding the meeting again. An event's details in **Calendar** list the notes about that meeting, and, for a recurring meeting, the notes from every other meeting in the series under **Series notes**, newest first.

## Note Details {#note-details}

The side panel beside a note shows what it's connected to:

- **Links** lists the conversations, tasks, and events the note links to; choose one to open it.
- **Checklists and tasks** lists the note's checklist items and tasks, unfinished ones first, with completed items grouped behind a count. Check an item off here, or use its menu to read its full text or convert it between a checklist and a Task.
- **Backlinks** lists the notes that link to this one, each with the line the link is on, written as you'd read it.
- **Unlinked** lists links in this note to notes that don't exist yet; choose one to create it.
- **People** lists the contacts the note links to, and links whose title is the name of a saved contact; choose one to open them in Contacts.

## Markdown Syntax {#markdown-syntax}

**Settings → Notes → Markdown Syntax** chooses how much of the Markdown you see. **Hide** shows only the formatting. **Hybrid** shows the Markdown characters around the cursor while you write. **Show** keeps them visible all the time. The choice applies to this device.

## Trash {#trash}

Press <kbd>#</kbd> in the list, or click the trash button beside the title, to move a note to Trash. **Trash** at the bottom of the list shows notes you've moved there. A note in Trash can be read but not changed; **Restore** brings it back, and **Delete Permanently** removes it from every device after you confirm.

## Import Your Notes {#import-your-notes}

To add one Markdown file, click **Import Note** beside **New Note**. Choose a `.md` or `.markdown` file; ThreeStrands adds it and opens it for writing. The title comes from `title:` in its front matter, then a leading heading, then the file name. If that title is already taken, a number is added. Each import makes a new regular note, leaving existing notes and daily notes as they are.

Linked pictures and files inside the Markdown file's folder come with it. Links outside that folder and pictures on the internet stay as links. If the note is too large or cannot be read, ThreeStrands shows the reason and leaves the current note open.

**Settings → Notes → Import Markdown Folder** adds the notes in a folder you choose, such as your Reflect folder or an earlier export from ThreeStrands. First ThreeStrands reads the folder and tells you how many notes, daily notes, and files it found, and lists anything it can't import and why. Nothing changes until you click **Import Notes**.

Files named by date in a `daily` folder become daily notes, and every other Markdown file becomes a note, titled by its first heading. Links between your notes keep working, and pictures and files the notes link to come along. Notes you already have are skipped, so importing the same folder again adds nothing and never replaces your notes. A note too long to sync is listed rather than cut short.

## Export Your Notes {#export-your-notes}

**Settings → Notes → Export to Markdown Folder** writes every note outside Trash to a folder you choose, as Markdown files that other apps such as Obsidian and Reflect can open. Daily notes go in a `daily` folder, named by date, the rest in `notes`, named by title, and their pictures and files in `attachments`, linked from the notes so they still open. Each file starts with a few lines naming the note, so a future version of ThreeStrands can match the files back to your notes.

Choose an empty folder. To update an export, choose the same folder again: ThreeStrands asks first, then replaces the files the last export wrote and leaves anything else in the folder alone. A folder that holds other files is refused.

Settings → Notes also shows how much of the space set aside for notes in cross-device sync they use, and how much their files take.

## Find a Note {#find-a-note}

Press <kbd>/</kbd> and type to search titles and note text. Search leaves out notes in Trash unless you check **Include Trash**; while you're in **Trash**, it searches only trashed notes. Press <kbd>Enter</kbd> to go to the results and keep your search, or <kbd>Esc</kbd> to clear it and go back to the list. Notes keeps your search and the open note when you switch to another view.

The command palette (<kbd>⌘/Ctrl + k</kbd>) finds notes too: matching notes are listed below the commands, and choosing one opens it in **Notes**.

## Notes on Your Other Devices {#notes-on-your-other-devices}

With [cross-device sync](sync.html) on, notes travel to your other devices, encrypted like the rest of your synced data. Update every device first: an earlier version stops receiving changes once you write a note.

A note can hold about 96 KB of text, and all your notes together about 3.5 MB, so there's room for everything else that syncs. If a note is too long, ThreeStrands says so and keeps your text.

Pictures and files travel separately, so they don't count toward that space; your notes can hold up to 2,000 of them. Another device downloads them after it syncs, and a picture appears in the note once it arrives. Files take space in your sync storage, such as your shared folder or bucket, and leave it when you delete their note from Trash.

If a note changes on another device while you're typing, ThreeStrands stops saving and asks which version to keep: **Keep My Version** or **Use Other Version**. If the note was moved to Trash there, **Restore and Keep My Text** brings it back with your text. If it was deleted there, you can keep your text as a new note.

If two devices change the same note before they sync, the note says so, names the other device, and stays read-only until you choose. **Compare** shows the lines that differ. **Keep This Version** or **Use Other Version** settles it on every device, and **Keep Both** saves the other version as a new note. The note list marks notes waiting for a choice.
