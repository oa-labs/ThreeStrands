---
title: Write Notes
nav: Notes
summary: Keep a note for each day and notes of your own, written in Markdown and synced between your devices.
reviewed: 0.207.1
---

## Open Notes {#open-notes}

Click **Notes** in the sidebar or press <kbd>4</kbd>. The list on the left starts with **Today**, then the notes you opened or changed most recently on this device. Notes reopens the note you had open last.

## Write {#write}

Select a note and press <kbd>Enter</kbd>, or click in it, to write. Press <kbd>Esc</kbd> to go back to the list, where <kbd>j</kbd> and <kbd>k</kbd> move between notes.

Notes are written in Markdown and formatted as you type: `**bold**` turns bold, `# ` starts a heading, and `- ` starts a list. Links open in your browser. Notes never load pictures or previews from the internet.

Paste or drag a picture or file into a note to add it. Pictures show in the note, and other files show as a small label with their size; click either to open it in its usual app. A note can hold files up to 20 MB each. A file stays with its note until you delete the note from Trash, even if you remove it from the text.

Today's note is created when you first write in it, so looking at a day leaves nothing behind. To make a note of your own, press <kbd>Shift</kbd>+<kbd>N</kbd> or click **New Note** and give it a title. Each title can be used once.

ThreeStrands saves a note a moment after you stop typing, and again when you leave it or close the app. Text you haven't saved yet stays on this device, so it's still there if the app closes unexpectedly.

## Link Notes {#link-notes}

Type `[[` to link to another note: pick one from the list, or keep typing a title that doesn't exist yet. Typing `@` at the start of a word does the same. `[[Title|other words]]` shows the other words but still links to Title, and `[[2026-11-09]]` links to that day's note. Click a link to open its note; a link to a note that doesn't exist yet creates it.

The panel on the right lists the notes that link to this one under **Backlinks**, each with the line the link is on. **Unlinked** lists links in this note to notes that don't exist yet; choose one to create it. **People** lists links whose title is the name of a saved contact; choose one to open them in Contacts.

When you rename a note, ThreeStrands changes the links to it in your other notes as soon as you leave the title. A note in Trash keeps its old links. So does a note that would become too long to sync, or one that was changed on two devices and is waiting for you to choose a version; ThreeStrands names those notes when it renames.

## Markdown Syntax {#markdown-syntax}

**Settings → Notes → Markdown Syntax** chooses how much of the Markdown you see. **Hide** shows only the formatting. **Hybrid** shows the Markdown characters around the cursor while you write. **Show** keeps them visible all the time. The choice applies to this device.

## Trash {#trash}

Press <kbd>#</kbd> in the list, or click the trash button beside the title, to move a note to Trash. **Trash** at the bottom of the list shows notes you've moved there. A note in Trash can be read but not changed; **Restore** brings it back, and **Delete Permanently** removes it from every device after you confirm.

## Export Your Notes {#export-your-notes}

**Settings → Notes → Export to Markdown Folder** writes every note outside Trash to a folder you choose, as Markdown files that other apps such as Obsidian and Reflect can open. Daily notes go in a `daily` folder, named by date, the rest in `notes`, named by title, and their pictures and files in `attachments`, linked from the notes so they still open. Each file starts with a few lines naming the note, so a future version of ThreeStrands can match the files back to your notes.

Choose an empty folder. To update an export, choose the same folder again: ThreeStrands asks first, then replaces the files the last export wrote and leaves anything else in the folder alone. A folder that holds other files is refused.

Settings → Notes also shows how much of the space set aside for notes in cross-device sync they use, and how much their files take.

## Find a Note {#find-a-note}

Press <kbd>/</kbd> and type to search titles and note text. Press <kbd>Esc</kbd> to clear the search, and again to go back to the list.

## Notes on Your Other Devices {#notes-on-your-other-devices}

With [cross-device sync](sync.html) on, notes travel to your other devices, encrypted like the rest of your synced data. Update every device first: an earlier version stops receiving changes once you write a note.

A note can hold about 64 KB of text, and all your notes together about 3 MB, so there's room for everything else that syncs. If a note is too long, ThreeStrands says so and keeps your text.

Pictures and files travel separately, so they don't count toward that space; your notes can hold up to 2,000 of them. Another device downloads them after it syncs, and a picture appears in the note once it arrives. Files take space in your sync storage, such as your shared folder or bucket, and leave it when you delete their note from Trash.

If a note changes on another device while you're typing, ThreeStrands stops saving and asks which version to keep: **Keep My Version** or **Use Other Version**. If the note was deleted there, you can keep your text as a new note.

If two devices change the same note before they sync, the note says so, names the other device, and stays read-only until you choose. **Compare** shows the lines that differ. **Keep This Version** or **Use Other Version** settles it on every device, and **Keep Both** saves the other version as a new note. The note list marks notes waiting for a choice.
