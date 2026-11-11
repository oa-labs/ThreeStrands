# Notes workspace design

**Status:** Built and shipped in 0.208.0: all five v1 steps with the
meowdown editor, [images and attachments](#images-and-attachments),
[Import Markdown Folder](#import-markdown-folder), and the
[linked notes and tasks proposal](#linked-notes-and-tasks-proposal).
So is the [meeting notes proposal](#meeting-notes-proposal).
What's still open is under [Later](#later).
**Date:** 2026-11-09, updated 2026-11-10
**Scope:** a Notes workspace for Markdown notes that travel through
replicated sync. The [linked notes and tasks proposal](#linked-notes-and-tasks-proposal)
below records the linked-notes and task integration built after v1.

Notes adds a fourth kind of content beside mail, calendar, and tasks: the
user's own Markdown notes, with a daily note, `[[wiki links]]`, and
backlinks. It borrows its model from [Reflect](https://github.com/team-reflect/reflect-open),
an MIT-licensed, open-source notes app built on a folder of Markdown files.
The workspace is laid out like Mail: a list of notes on the left, the open
note in the middle, and the context panel on the right.

This document records the decisions for v1, what v1 leaves out, the
order of work, and the features built on top of v1.

## Decisions

- **Notes are synchronized records, and Markdown is their text format.**
  Each note is one replicated entity whose body is the note's Markdown
  source. The user gets real files through **Export to Markdown Folder**,
  which writes Reflect's layout. A watched folder that is the source of
  truth, as in Reflect, is left for v2 (see [Later](#later)).
- **The editor is a WYSIWYG Markdown editor.** Reflect's editor hides
  Markdown syntax by default. It can also reveal syntax around the cursor or
  show it all the time. v1 matches that behavior; see [Editor](#editor) for
  the candidate library and the rules any editor must follow.
- **Import reads the export's shape.** The export layout and front matter
  were chosen so an import reads the same shape. Import was planned for v2
  and is now built; see [Import Markdown Folder](#import-markdown-folder).

## Why records rather than a folder

Replicated sync is not file sync. Each device publishes one encrypted,
mergeable state that holds all of its synchronized data
([replicated sync design](replicated-sync-design.md)). A folder of files as
the source of truth would need a file watcher, reconciliation of edits made
by other editors, rename detection, and coexistence with whatever Dropbox or
iCloud does to the same folder. That is a second sync system beside the one
the app already has. Records keep one merge model, one conflict model, and
one encryption boundary, and the export still gives the user plain files
they own.

## Sync constraints

These limits shape most of the v1 design:

| Limit | Value | Where |
| --- | --- | --- |
| One field value | 96 KB (`MAX_VALUE_BYTES`) | `crates/sync-envelope/src/limits.rs` |
| One snapshot, compressed | 4 MB (`MAX_COMPRESSED_BYTES`) | same |
| One snapshot, decompressed | 32 MB (`MAX_DECOMPRESSED_BYTES`) | same |
| Republishing | every change reseals and uploads the whole snapshot | `replicated_sync.rs` |

Tasks, contacts, Split Inboxes, and every other synchronized type share the
snapshot budget with notes.

## Data model

### Replicated entity

Add `EntityType::Note` as the last variant in
`crates/sync-protocol/src/lib.rs`. The derived order is the snapshots'
canonical field order, so a new variant must go last.

| Field | Meaning |
| --- | --- |
| `_entity` | presence, as for every entity |
| `id` | the entity id |
| `title` | the note's title; for a daily note, its date |
| `body` | the Markdown source, without front matter |
| `dailyDate` | `YYYY-MM-DD` for a daily note, absent otherwise |
| `createdAt` | when a regular note was created; absent for a daily note |
| `trashed` | `true` while the note is in Trash |

Fields a later version adds are accepted and ignored, so it can add one
without older devices refusing its notes.

- **One field per concern.** Title, body, and trash state are separate
  fields, so renaming on one device while editing on another doesn't
  conflict.
- **No synchronized timestamps that change.** Two devices that touch the
  same note would always write different times, and the field would show as
  a conflict nobody cares about. So the time a note last changed, and when it
  went to Trash, are kept by each device for itself. Trash is the boolean
  `trashed`, so trashing on two devices at once agrees. A daily note has no
  `createdAt`, because its date says when it began and two devices starting
  the same day would otherwise disagree.
- **The body is one field.** Don't split a long body across several fields.
  Each field merges separately, so concurrent edits could combine halves of
  two versions into a note neither device wrote.
- **Daily notes share one ID.** A daily note's entity id is
  `daily:YYYY-MM-DD`. Two devices that open today's note while offline
  create the same entity, so they merge into one note, with at most a body
  conflict, rather than two duplicates. Other notes use a random id.
- **Trash is a field, not a deletion.** In this sync model a deletion beats
  a concurrent edit and keeps the entity deleted. If **Delete** removed the
  entity, an edit made on another device at the same time would be lost
  silently. Setting `trashed` keeps that edit recoverable. A permanent
  **Delete from Trash** is an ordinary replicated deletion. Deleting a note
  the replica already holds is recorded even while sync is off, so the note
  doesn't come back when sync resumes.
- **Titles.** A regular note's title is unique among notes outside Trash
  (ignoring case and repeated spaces), at most 200 characters, and can't
  contain `[`, `]`, or `|`, which end or split a link. It can't be a date,
  which names that day's daily note. A daily note's title is its date and
  can't be changed. If devices independently use the same title, the note
  with the lowest id keeps it and the others receive numbered titles, within
  the same 200-character limit. All authored titles are reserved before
  assigning numbers. This is a deterministic local projection, not a new
  replicated title edit: body edits never publish the generated name. Existing
  title-only links keep their text and resolve to the note keeping the original
  title; their intended target cannot be inferred across offline creations.

### Size limits

The limits live in `crates/sync-protocol` beside the other synchronized
types' limits.

- **Per note.** `MAX_NOTE_BODY_BYTES` is the envelope's `MAX_VALUE_BYTES`
  (96 KB, raised from 64 KB before Notes first shipped), measured on the body's JSON encoding (`note_body_bytes`), since
  that is what the envelope measures. Escaping grows the text (newlines,
  quotes, and backslashes each take two bytes), so a shorter body can be
  over. A save past the limit is refused with "This note is too long to
  sync"; the editor keeps the text and never truncates it.
- **All notes together.** `MAX_SYNCED_NOTES_TOTAL_BYTES` is 3.5 MiB (raised
  from 3 MiB before Notes first shipped), counted on the encoded fields of
  every note, Trash included. The snapshot holds at most 4 MB after
  compression, shared with every other synchronized type. Note text
  compresses well (a real 1.1 MB set of notes came to about 400 KB), and even
  base64 that encodes a pasted image or key compresses by about a quarter,
  so 3.5 MiB of notes stays under 2.7 MB compressed and leaves room for
  everything else. A change that would grow notes
  past it is refused; a change that doesn't grow its note always saves, so a
  user over the budget because of notes from another device can still trim.
  A failed seal would stop every other synchronized type from syncing too.
  **Settings → Notes** shows the usage (`notes_storage`).
- **The lasting fix** is splitting snapshots by entity type, already listed
  under the replicated sync design's known gaps. It is not part of v1.

### Write cadence

Every replicated write reseals and uploads the whole snapshot, so the editor
must not write one per keystroke.

As built, a text save (`update_note`, `save_daily_note`) records the change
in the replica but doesn't start a sync; the periodic sync (every 30
seconds) carries a burst of edits in one upload. Creating, trashing,
restoring, deleting, and importing notes still sync at once. Quitting runs
one more sync, bounded to 5 seconds (`finish_exit`); anything it doesn't
finish stays recorded and goes on the next launch.

- A save writes the `notes` row and the replica together, so the editor
  saves after about two seconds without typing, when it loses focus, when
  the user opens another note or workspace, and when the window closes.
- Until then the editor keeps the unsaved text itself (a per-device draft),
  so a crash loses at most what wasn't kept there.
- Saving text identical to what is stored writes nothing, and saving an
  empty body for a day without a note doesn't create it. Opening or reading
  a note never writes. See the editor rules below.
- Every save names the version it started from (the note's `updatedAt`, or
  none for a day without a note). If the note changed or was deleted on
  another device since, the save writes nothing and returns the note as it
  is now (`NoteSave { saved: false }`). Without this, a save would replace
  an edit that arrived while the user was typing, with no conflict to show
  for it, because the replica had already seen that edit. The editor then
  stops saving, keeps the user's text, turns read-only, and offers **Keep My
  Version** (save over it) or **Use Other Version** (drop the typing). For a
  deleted note the choices are **Keep as New Note** and **Discard My Text**.
- A note in Trash is never current, even though trashing changes no text
  and so leaves `updatedAt` alone: a save over it returns the trashed note
  instead of trying the edit. The editor then offers **Restore and Keep My
  Text** (restore, then save the typing over the restored note) or
  **Discard My Text** (show the note in Trash). If the restore is refused,
  such as when another note took the title, the reason shows and both
  choices stay. A draft left in a note that went to Trash while the app was
  closed asks the same way instead of retrying the save.
  A draft found on reopening that started from an older version gets the
  same choice.
- The app's exit waits for registered before-exit tasks (`beforeExit.ts`)
  after the composer saves its draft, so the open note saves before the app
  closes.

### Local tables

All of these live on one device and are rebuilt from the replicated notes.
None of them travel through sync or settings export.

- `notes`: the materialized notes, written by the projection path like other
  synchronized types. A local change writes the row and records its changed
  fields in the replica in one transaction. While sync is off, the changed
  field names wait in `replica_dirty_fields`; sync records them before it
  pulls, so they are concurrent with a peer's edits rather than replaced by
  them.
- `note_links`: one row per `[[link]]` per note (up to 500), with the target
  title normalized (case folded and whitespace collapsed) and the line it
  sits on. `[[Title|shown text]]` links to Title. Links inside fenced code
  blocks or inline code, and escaped ones (`\[[`), don't count.
- `note_search`: an FTS5 table over title and body, like `thread_search`.
- `note_visits`: when this device last opened each note, for the
  **Recent** list.

Recently opened order belongs to the device. Two devices show different
Recent lists, and that is intended.

### Compatibility

A device on an earlier version can't open a snapshot that holds a note. It
stops receiving changes from updated devices until it is updated, and
nothing is lost meanwhile. [Cross-device sync](cross-device-sync.md#notes)
lists Notes in its data boundary, and the 0.208.0 changelog tells users to
update every device before writing notes.

### Settings export

Notes are content, not settings. They stay out of the encrypted settings
export, so the transfer format and its compatibility rules are not touched.
Export to Markdown Folder is how notes leave the app.

## Conflicts

Edits to the same note on two devices before they sync leave two values in
the `body` field. Every device shows the same working value, the one with
the greatest `(lamport, device, counter)`, and keeps the other for review.

Settings already lists sync conflicts under **Resolve Conflicts**, through
`FrontierConflictEditor`. That works for short values, but a note body is
too long to compare there. So v1 handles note conflicts in the note:

- A banner above a conflicted note says it was also edited on another
  device, and names the device.
- **Compare** shows the other version beside the current one, with changed
  lines marked.
- **Keep This Version** and **Use Other Version** resolve the conflict with
  an ordinary write, through the same command Settings uses.
- **Keep Both** writes the other version into a new note named
  "<title> (from <device>)", and keeps this one (`keptBothTitle`). The title
  part is shortened so the name fits 200 characters, `[`, `]`, and `|`
  become `-`, and a name already in use gets " 2", " 3", and so on. The
  banner remembers the note it made, so trying again after the conflict
  failed to resolve finishes the job rather than making another copy.
- The note list marks conflicted notes, and the generic Settings list links
  a note conflict to the note rather than rendering its whole body.
- With three or more devices in a conflict, the banner offers each other
  device's version in turn. A version is that device's value for every
  field in conflict and the shown value elsewhere.
- **Compare** aligns lines up to `MAX_DIFF_CELLS` line pairs; past that it
  marks everything between the shared start and end as changed.

A local write replaces every value of the field it writes, so typing into a
conflicted note would settle the conflict by discarding the other device's
version. The storage layer refuses an edit to a field in conflict ("This note
was also changed on another device. Choose which version to keep before
editing it."), so the editor stays read-only under the banner until the user
chooses. Resolving applies the chosen value on this device at once, as a
merge would.

v1 doesn't merge concurrent text edits character by character. That would
need a text CRDT inside the field, which the snapshot model and the 96 KB
field limit don't fit.

## Workspace UI

Notes follows the layout and command rules in [DESIGN.md](../DESIGN.md#workspaces).

### Navigation

- Add `"notes"` to `RightWorkspace` in `src/useWorkspaces.ts`, so the active
  workspace stays one state value.
- `4` opens Notes. Mail, Calendar, Tasks, and Notes are the core views, so
  they take `1` through `4`. Contacts, Files, and Agents are used less, and
  each moves up one key. The sidebar lists the views in key order:

  | Key | Before Notes | With Notes |
  | --- | --- | --- |
  | `0` | Today | Today |
  | `1` | Mail | Mail |
  | `2` | Calendar | Calendar |
  | `3` | Tasks | Tasks |
  | `4` | Contacts | **Notes** |
  | `5` | Files | **Contacts** |
  | `6` | Agents | **Files** |
  | `7` | — | **Agents** |

  The new keys ship in the same release as the Notes workspace, so `4` never
  opens an empty view and users relearn the keys only once. That release's
  changelog says the Contacts, Files, and Agents keys moved, and DESIGN.md's
  list of primary commands gets the new keys.
- Register the keys and the palette commands below in `src/commands.ts`, the
  central command registry:
  - **Go to Notes View** (`4`)
  - **Open Today's Note**, from anywhere
  - **New Note** (`Shift+N`), in Notes, like New Contact in Contacts
  - **Search Current View** (`/`) focuses the note search in Notes
- Opening the workspace doesn't steal focus or open a form. It selects the
  last open note on this device, or today's note.

### Note list (left)

- **Today** is pinned at the top. Selecting it creates the daily note only
  once the user types, so browsing never writes an empty note.
- **Recent** comes next: notes opened on this device, newest first. Each
  row's preview is the note's text without Markdown syntax (`plain_text`
  natively, `notePlainText` in the page, kept to the same cases).
- A search box above the list searches titles and note text as the user
  types, through the local search index, and lists the matches in place of
  the list. The field uses the shared search styling and magnifying glass.
  `/` focuses it without taking focus when Notes opens or returns. Enter
  returns focus to the results while retaining the query; Escape clears the
  query and returns focus to the list. Query, Notes/Trash view, and Include
  Trash are kept for the window session across workspace switches, alongside
  the existing last-selected-note restoration. Explicit Open Note/Today
  requests still clear search and return to Notes to reveal their target.
- Search excludes Trash by default. **Include Trash** adds clearly marked,
  read-only trashed results. Within **Trash**, search matches only trashed
  notes. Moving between Notes and Trash retains the query and the Include
  Trash preference.
- `j`/`k` and Up/Down move through the list, as in Mail. Space and
  Shift+Space page through the open note, as they page a message. Enter
  starts writing in the open note from anywhere in Notes outside a text
  field.
- **Trash** sits at the bottom. A trashed note opens read-only with
  **Restore** and **Delete Permanently**, which confirms first, with Cancel
  focused. Restoring a note whose title another note took meanwhile is
  refused until one is renamed. Today's note can go to Trash too; Today
  stays selected and shows it read-only.

### Editor (center)

- The title is the first line, edited in place. A daily note's title is its
  date, shown with the weekday, and it can't be renamed.
- The body is the editor described below, in `NoteEditor`, at the reading
  type size. Blocks sit close together, as in Reflect: list items
  `--space-1-5` apart, paragraphs `--space-2`, with more room above
  headings; the app's stylesheet overrides meowdown's 0.85em block gap.
- meowdown's menus are browser popovers that stay open while closed; the
  stylesheet hides closed ones and clears the browser's popover border,
  which otherwise shows as an empty box in the middle of the window.
- A button beside the title, and `#` in the list, move the note to Trash.
- A quiet status beside the Trash button shows "Saved" for two seconds
  after a save (not when a note is merely opened), or "Too long to sync".
  The conflict banner shows above the note when there is one.

### Context panel (right)

Use `WorkspaceContextRail`, with `ContextSection` and `ContextRow` for every
list, under the panel rules in DESIGN.md: empty sections are left out, lists
show three rows before **Show more**, and headings collapse. Step 2 shows
**Note details** (when the note was created, and last changed on this
device), and the sections below follow it.

Two sections come before Note details, borrowed from Reflect's sidebar:

- **Calendar**: the month, headed by its name, with Previous Month, **Jump
  to Today** (`t`), and Next Month. Choosing a day opens that day's note,
  which, like Today, writes nothing until the user types. The month follows
  the open daily note; paging it opens nothing. The week starts on the
  locale's first day, and the grid shows only the weeks that hold the
  month's days, so no row belongs wholly to another month. Neighbouring
  months' days recede and carry no marks. Today is shown by an accent,
  semibold number; only the open daily note's day is filled, so today never
  looks chosen. A dot marks each day of the month whose note has text
  outside Trash (`list_daily_note_dates`, read for the weeks shown and again
  after a save, a Trash change, or a sync), so a day emptied again loses its
  mark. The grid is `MonthGrid`, shared with the date field's picker. In the
  rail it uses narrow weekday names and small rows. On the page it takes
  focus only from its own arrow keys, never when it appears.
- **Agenda**, on a daily note only, when the user has calendars: that day's
  events from `CalendarAgenda`, each with **Add Note**, which opens the
  meeting dialog for the event as the other agendas do. The added line
  appears in the open note, which saves any typing first. Selecting an event
  opens it in Calendar. A regular note has no day, so it shows no agenda.

A daily note's title has **Previous Day** and **Next Day** arrows beside it.
In Notes, `-` and `=` step a daily note back and forward a day, as they step
Calendar's days, and do nothing on a regular note; `t` opens today's note, as
it goes to today in Calendar. A continuous scrolling journal of days, as in
Reflect, is left for later (see [Later](#later)).

- **Backlinks**: notes that link here, each with the line around the link as
  its detail. The line is shown as text, as note previews are: a meeting
  line reads "3:00 pm met with Dana for OA Monthly", not its Markdown. Lines
  indexed before this rule are rebuilt on upgrade (schema v107).
- **Unlinked**: `[[links]]` in this note whose target doesn't exist yet.
  Selecting one creates that note.
- **People**: `[[Name]]` links whose title matches a saved contact's name.
  Selecting one opens the contact in Contacts. The first 30 distinct links
  are looked up, each against the contact search, matched by normalized
  name.

### Wiki links

- Typing `[[` opens a title picker with existing notes
  (`suggest_note_titles`: regular notes outside Trash whose title contains
  the typed text, prefix matches first, then recent). Typing a new title
  and pressing Enter links to a note that doesn't exist yet. meowdown also
  opens the picker for `@` at the start of a word, and inserts the same
  `[[Title]]` link.
- `[[Title|shown text]]` shows the shown text and follows Title.
- Links resolve by normalized title. Daily notes resolve by date
  (`[[2026-11-09]]`).
- Selecting a link opens its note, or creates it if missing.
- Renaming a note rewrites `[[Old Title]]` to `[[New Title]]` in every note
  that links to it, as ordinary edits on the renaming device, in the same
  transaction as the rename. Each rewritten note counts toward the size
  limit, and one that would pass it keeps the old link and is listed in the
  rename's notice (`NoteSave.relink`). So is a note whose body is in
  conflict: rewriting it would settle the conflict. Notes in Trash keep old
  links. A change of case or spacing only rewrites nothing, since links
  match by key.
- A title saves when the title field is left, not after the typing pause,
  so a rename rewrites links once rather than for every half-typed title.
  The typed title is kept in the draft meanwhile.

### Search

Note search uses the local FTS5 index. **Search Current View** searches
notes while Notes is open. The command palette (`⌘K`) also lists up to five
matching notes below the matching commands, each with its highlighted
excerpt, from anywhere in the app; choosing one opens it in Notes.

## Editor

### Reflect's editor

Reflect uses **meowdown** (`@meowdown/react`, `@meowdown/core`,
`@meowdown/markdown`), an MIT-licensed Markdown editor by the author of
ProseKit. It is built on ProseMirror through ProseKit, parses and writes
Markdown with remark, and uses CodeMirror's language package for code
blocks. Its "mark mode" has three settings, which Reflect exposes as
**Markdown syntax: Hide / Hybrid / Show**:

- **Hide**: full WYSIWYG, syntax never visible (Reflect's default).
- **Hybrid**: syntax appears only around the cursor, like Typora or
  Obsidian's live preview.
- **Show**: syntax always visible, still styled.

It also brings the `[[` menu, wiki-link hover cards, a slash menu, tags,
images, KaTeX math, and embeds.

### Candidate and review

meowdown is the first candidate, because it already does what Reflect
users expect. Before adopting it, run the [dependency
safety](../DEVELOPERS.md#dependency-safety) review, `cargo vet`, and
`cargo deny`, and note these concerns for the developer:

- **Churn.** It is pre-1.0, with about 160 releases since June 2026. The
  newest (0.84.0) is a day old, so pin an exact version that is at least two
  weeks old.
- **Weight.** It pulls in ProseKit, ProseMirror, the unified and remark
  stack, KaTeX, `@base-ui/react`, and embed packages. Measure the bundle
  growth, and turn off what v1 doesn't use: embeds that fetch remote content,
  math, and images.
- **Remote content.** Its embeds and link previews (YouTube, X posts) fetch
  from the network. v1 disables them. See the rules below.

If the review rejects meowdown, use ProseKit or Tiptap directly with a
remark-based Markdown parser and serializer. The rules below apply to any
choice.

**Adopted:** `@meowdown/react` 0.81.2 and `@meowdown/core` 0.81.3, pinned
exactly. Both, and `@meowdown/markdown` and `@meowdown/embed`, carry SLSA
provenance. Everything they add loads with the Notes chunk or later:
Notes itself is about 1 MB, Mermaid (1.5 MB) and KaTeX (256 KB) load only for
a diagram or math, and each code-block language loads on first use. The
startup bundle is unchanged. `NoteEditor` turns off every remote resolver
(images, X posts, YouTube) and embed paste; the app's CSP blocks remote
images and connections behind that. Upgrades follow the same rules: an exact
version at least two weeks old.

### Rules for any editor

- **Markdown text is the stored value.** The editor's document model is a
  view of `body`, never a second stored format.
- **Loading never writes.** A WYSIWYG editor normalizes Markdown when it
  parses and serializes (list markers, emphasis characters, blank lines).
  Write only after a user edit. Otherwise opening a note on two devices
  makes two different "edits" and a conflict nobody made.
- **Round-trip tests.** Fixtures for headings, nested lists, task lists,
  tables, code blocks, links, wiki links, and front matter must survive load
  and save unchanged. Where normalization is unavoidable, the test states
  the exact output. meowdown keeps all of them but tables, whose delimiter
  row it writes as `| --- |`; that happens only when the user edits the note.
- **No raw HTML.** Note text comes from other devices and from pastes, so it
  is untrusted. HTML in Markdown renders as text or is dropped, never as
  live markup.
- **Links open outside the app** through the opener plugin, after the same
  safety check mail links get: only `http`, `https`, and `mailto` open, and a
  click on a link never navigates the app's own window.
- **No remote fetches.** Remote images stay blocked in v1 and never load
  directly. Images in v2 must go through the existing native proxy and
  blocked-source flow, as in mail.
- **The editor reports its mode.** **Settings → Notes → Markdown Syntax**
  offers Hide, Hybrid, and Show, stored as a device preference.

## Export to Markdown Folder

**Settings → Notes → Export to Markdown Folder** writes every note that
isn't in Trash to a folder the user picks:

```text
<chosen folder>/
  daily/2026-11-09.md
  notes/<title>.md
```

- Titles become file names with characters that are unsafe on macOS,
  Windows, or Linux replaced. Clashes get a numeric suffix.
- Each file starts with front matter holding `id`, `created`, and `updated`,
  so a v2 import can match files back to notes.
- `[[wiki links]]` are written as typed, which Reflect and Obsidian both
  read.
- Export writes only inside the chosen folder, never follows symlinks out of
  it, and refuses a folder that isn't empty unless the user confirms
  replacing an earlier export there.

As built (`src-tauri/src/notes_export.rs`):

- The native folder picker's choice stays native-side; the page never sends
  a path, so an export writes only where the user pointed the picker.
- An export leaves `.threestrands-notes-export.json` listing the files it
  wrote. A folder holding only that list's export (and `.DS_Store` or
  `Thumbs.db`) can be replaced after confirming; replacing removes only the
  listed files that are still ordinary files inside the folder, and keeps
  anything else. A folder holding anything else is refused. The list is
  written before the notes, so an export that stops partway can still be
  replaced.
- Every file is created new (`create_new`), so nothing existing is
  overwritten or written through, including a symbolic link; a `daily` or
  `notes` that is a link or a file is refused.
- File names replace `/ \ : * ? " < > |` and control characters with `-`,
  replace a leading dot and trailing dots or spaces, add `-` to Windows
  device names, and are cut to 200 bytes (`MAX_FILE_STEM_BYTES`) at a
  character boundary. Clashes, ignoring case, get " 2", " 3", and so on.
- Front matter values are JSON-quoted strings: `id`, `created`, and
  `updated` as this device has them.

**Settings → Notes** also shows how much of the notes' sync space they use.

- Attachments go in `attachments/`, named by their file names with the same
  rules and clash numbering, and are listed in the manifest like notes. A
  note's `attachment:<id>` link destinations become
  `../attachments/<percent-encoded name>`; only a whole destination right
  after `](` changes. Files this device hasn't downloaded yet are left out,
  keep their `attachment:` links, and are counted in the result.

## Import Markdown Folder

**Settings → Notes → Import Markdown Folder** adds the notes in a folder the
user picks: a Reflect folder, or one Export to Markdown Folder wrote
(`src-tauri/src/notes_import.rs`).

- **Two steps.** Choosing the folder reads it and builds the whole plan
  without writing; the page sees a summary (notes, daily notes, files,
  notes already here, and what won't be imported, each with its reason) and
  imports only on **Import Notes**. As with export, the folder stays
  native-side.
- **Layout.** A `YYYY-MM-DD.md` in a folder named `daily` is that day's
  note; every other `.md` file, in any folder (Reflect's `notes`, `person`,
  `meetings`, `collection`, `archive`), is a regular note. Hidden entries
  (`.git`, `.reflect`) and symbolic links are skipped, and Reflect's
  `*.reflect.md` picture descriptions aren't notes. At most 20,000 entries
  and 5,000 notes are read.
- **Titles.** A leading `# Heading` is the title and leaves the text; else
  `title:`; else the file name. `[`, `]`, and `|` become `-`, a date gets
  " (note)", and a title in use (here or earlier in the folder) gets " 2",
  " 3", and so on.
- **Ids and re-importing.** A daily note's id is its date. A front matter
  `id` that is a ThreeStrands id (a UUID) is kept, so an export comes back
  as the same notes; any other `id`, or the path when there is none, is
  hashed into `import-<32 hex>`. A note whose id is already here is skipped
  and counted, so importing the same folder again adds nothing, and nothing
  is ever replaced. Two files with one `id` (iCloud's "name 2.md") are
  imported once when their text matches, and both, under different ids,
  when it doesn't.
- **Dates** come from `created` and `updated` when they are RFC 3339 times
  (an export's), else the file's modification time.
- **Links.** `[[name]]` naming a note's file name becomes `[[Title]]`, and
  one naming an alias becomes `[[Title|alias]]`. A relative link to a note
  file (`[text](../notes/Plan.md)`) becomes `[[Title|text]]`; one whose path
  went stale finds its note by file name when only one note has it. Fenced
  code, inline code, and web links are left alone.
- **Files.** A link or image whose destination is a file inside the folder,
  relative to the note or to the folder's root (Reflect writes
  `assets/…`), becomes an attachment, one per file per note. Nothing outside
  the folder, and nothing through a symbolic link, is read; each file is
  checked again when it is read for the import. A file over 20 MB keeps its
  link and is reported.
- **Front matter.** A leading `---` block is front matter only when every
  line in it is `key: value`, blank, or an indented or `- ` line continuing
  a key; otherwise the `---` is a horizontal rule and the whole file is the
  note's text, so nothing is dropped. `id`, `title`, `created`, `updated`,
  `summary`, `snippet`, `summary_source_hash`, and `pinned` are dropped;
  anything else (such as `aliases`) stays at the top of the note.
- **Not imported**, with the reason shown: a note over 96 KB (never cut),
  a file that isn't UTF-8, Reflect's conflict copies (`name.md~other
  device`), and a matching second copy. A note that can't be saved during
  the import (notes out of sync space, a title taken meanwhile) is listed
  afterwards. Each note and its files are saved in one transaction
  (`import_note`), and recorded for sync like a new note.

## Images and attachments

Pasting or dropping a file into a note stores it and inserts
`![](attachment:<id>)` for an image or `[name](attachment:<id>)` for any
other file (meowdown's `onFilePaste` decides which). `<id>` is 32 hex
digits. The note's text holds only that reference, so a file never counts
toward the note's 96 KB or the notes' 3.5 MiB.

**Files travel separately from snapshots.** The sync design's whole-state
snapshots are resealed on every change, so carrying files in them would
re-upload every file on every keystroke's save. Instead:

- **The description is replicated.** `EntityType::NoteAttachment` holds
  `id`, `noteId`, `name`, `mimeType`, `size`, `sha256`, `createdAt`,
  `sealedBy` (the sealing device), and `index`, the CID of an object listing
  the file's sealed parts. Each description takes about 400 bytes of the
  snapshot, and notes hold at most `MAX_SYNCED_NOTE_ATTACHMENTS` (2,000)
  files. Like every new entity type, a device on an earlier version can't
  open a snapshot that holds one.
- **The bytes are sealed objects of their own.** The file is split into
  parts of at most `MAX_ATTACHMENT_PART_BYTES` (3 MiB, so a part that
  doesn't compress still fits a message), and each part is sealed as
  `ObjectKind::AttachmentPart`: chunked, padded, encrypted under the current
  epoch key, and signed by the sealing device under its own signature
  domain. Its message id is derived from the attachment id and part number,
  and opening checks both. A file holds at most
  `MAX_NOTE_ATTACHMENT_BYTES` (20 MiB).
- **Sealing waits for sync.** A file is recorded in the replica only once it
  is sealed, during the next push, so another device never learns of a file
  it can't fetch. The chunks go through the same delivery queue as
  snapshots, with `state_sequence` NULL, so they never hold up a head, and
  the repair sweep copies them to a newly added connector.
- **Downloading follows a pull.** A device fetches files it doesn't have,
  up to 64 MiB per cycle. Every object is checked against its CID, each part
  against its signer, attachment, and position, and the whole file against
  `size` and `sha256` before it is stored. A failure is retried after a
  minute, doubling to six hours. The sealing device's key is used whether
  or not that device was since removed: the objects are pinned by a snapshot
  an active device vouched for.
- **Deletion follows the note.** Deleting a note from Trash deletes its
  attachments on every device. A file added on one device while another
  deleted its note is swept up after the next pull
  (`delete_orphaned_note_attachments`): a regular note's file whose note is
  in neither the `notes` table nor the replica is deleted, and the deletion
  is recorded for every device. A daily note's files stay, since today's
  note can hold a file before it is first saved. Removing a reference from the text keeps the
  file, so undo and older versions still show it. The sealing device
  retires its objects through `sync_retired_objects`; they leave each
  connector once a snapshot without the attachment is published there.
- **Leaving a group** keeps the files this device has and seals them again
  for the next group; descriptions of files it never downloaded go.

**Showing files.** The editor's `resolveImageUrl` asks the native side for
an attached image as a `data:` URL, which the CSP already allows. A file is
an image only if its bytes are PNG, JPEG, GIF, WebP, or AVIF; the declared
type is never trusted, and SVG is never shown. The bytes are checked again,
with the mail image dimension limits, before each display. Remote image
sources still resolve to nothing. An image another device added waits and
appears once a sync brings it. Other files show as meowdown file pills with
their size. Clicking a file or image opens it in its default app through
the mail attachment path: the same warning for risky file types, a
quarantined copy in the reader cache.

**Settings → Notes** counts attached files and their size apart from the
notes' sync space.

**Files lists them too.** With `FileQuery::include_notes`, `list_files`
merges notes' files (outside Trash, today's unsaved note included) with mail
attachments, newest first by when each was sent or added: both lists are
read to the end of the requested page and the merged window is returned.
Files asks for them only while it covers every account, and the query
leaves them out when it names a conversation, sender, direction, the
calendar type, or a message to put first, since only mail has those. A
note's file carries `note` (id, title, daily date) instead of the mail
fields, and `availability` `syncing` until another device's file arrives.
Search matches the note's title as it matches a mail sender. Stars live in
`note_attachments.starred` (v102), on this device only. Previews, Open,
and Save as… go through the same native preview and quarantine paths as
mail attachments.

Known gaps:

- The sealing device keeps a sealed copy of each file (in `sync_objects`)
  so a new connector can be filled, so files take about twice their size on
  that device while sync is on.

## Security and privacy

- Notes are encrypted in snapshots like every other synchronized type.
  Storage providers see only ciphertext.
- Note text never goes into crash reports or diagnostics.
- v1 sends notes to no AI provider. If v2 adds "ask your notes", it needs
  its own consent, and a note-level private flag like Reflect's
  `private: true`.

## Tests

- **Rust:** two-device convergence for notes (direct merges, plus one
  push and pull through the fake transport with a body at the limit); daily notes
  created offline on two devices merge into one; edit racing Trash keeps the
  edit; edit racing permanent delete keeps the note deleted; conflict
  resolution through the shared command; encoded-size limit below, at, and
  above the limit; projection fills `notes`, `note_links`, and
  `note_search`; export file names, collisions, and folder containment.
- **Frontend:** the editor's round-trip fixtures; loading a note doesn't
  write; the write cadence; the conflict banner and its actions; rename
  rewriting links; the backlinks and unlinked sections. Test hooks and
  components directly, and keep one slim `<App />` test for the `4` shortcut
  and palette wiring. Existing App tests that press `4`, `5`, or `6`, or find
  the sidebar buttons "Contacts (4)", "Files (5)", and "Agents (6)", change
  with the new keys. That is a deliberate contract change.
- **Security-negative:** raw HTML, `javascript:` links, and remote image
  syntax in a note body never render live or fetch, even with attachments
  on; a file claiming an image type without an image's bytes is stored as
  `application/octet-stream` and never shown; a forged or corrupted part is
  refused.
- **Attachments:** a file of more than one part crosses the fake transport
  and arrives byte for byte; the snapshot carries only the description;
  deleting the note removes the objects from the connector; size and count
  limits below, at, and above; export file names and rewritten links.

## Order of work

1. **Sync and storage.** The entity type, local tables, projection, search
   index, size limits, and daily-note ids, with the Rust tests above.
2. **Workspace shell.** The workspace, list, editor, write cadence, today's
   note, and the new view keys. Built with the plain text area; adopt the
   WYSIWYG editor after its dependency review.
3. **Links.** Wiki links, backlinks, unlinked links, people, and rename
   rewriting.
4. **Safety.** The conflict banner and Trash.
5. **Export.** Export to Markdown Folder.

Each step ships with its tests. Steps 2 through 5 each add a changelog entry
under `### New` or `### Improved` as they become visible.

## Linked notes and tasks proposal

**Status: implemented and shipped in 0.208.0.** This extends the built Notes
workspace. Where the built UI differs from the proposal, the
[implementation notes](#implementation-notes) say so.
It covers **Add to Today's Note** from Mail, explicit links between notes
and conversations, tasks, and calendar events, and Markdown tasks in the
existing **Tasks** workspace. It adds no AI processing.

### User contract

- Capture a conversation in today's note without leaving the email being
  read. Return to that exact conversation after opening the note.
- Insert a link to a conversation, task, or event in any note. See the
  referring notes from the linked item's context panel.
- Checklists stay in their notes as square checkboxes. Tasks use round
  checkboxes and appear in **Tasks**; checking a task in either place
  changes the same item. Users can explicitly convert between the two.
  Merely linking to an existing task creates no new task.

### Add to Today's Note

Add **Add to Today's Note** to the conversation actions menu and command
palette, enabled for the focused conversation. No default keyboard shortcut
in this release. It appends a bullet containing the subject as an internal
conversation link; an empty subject displays **(No subject)**. Escape
Markdown punctuation in the label. Do not copy message HTML, quoted text,
attachments, or an AI summary. The result is a reference, not a task:

```markdown
- [Project proposal](threestrands:conversation/<encoded-reference>)
```

- Resolve today's date in the device's local timezone once when the action
  starts. Use the existing `daily:YYYY-MM-DD` identity, including offline.
  Crossing midnight while the action saves does not change its destination.
- Append at the end, separated from existing text by a newline. If the body
  ends inside an unclosed fenced code block, refuse the append and offer
  **Open Today's Note**; never hide the capture inside code or repair the
  user's Markdown automatically.
- On success, stay in Mail with the same focused message and expansion
  state. Announce **Added to today's note**, with **Open Note** and **Undo**.
  Opening selects the inserted reference. Returning to Mail restores the
  conversation and message focus. The action does not mark mail read.
- Repeating the action for a conversation already linked in that day's
  saved note adds nothing; show **Already in today's note** with **Open
  Note**. Compare canonical targets, not labels. A deliberate second link
  typed by the user is allowed.
- Flush an open daily-note draft before appending, using the existing save
  guard. If it cannot save, leave the draft intact and refuse the append.
  A dirty draft found while its editor is closed also requires resolution
  before capture; never append behind an unsaved draft.
- Use a version-checked native transaction to create/append and rebuild the
  indexes. A stale version returns a retryable result, without overwriting
  text. Retry only after reading the new body and checking for a duplicate.
  A body conflict, Trash, permanent deletion racing the write, or a size
  limit refuses the action with the existing resolution/open-note path.
  Do not restore or recreate a deleted note silently. Show success only
  after the local transaction commits; offline sync may follow later.
- **Undo** removes only this capture, with the same version check. If the
  note has changed since capture, offer to open it instead of restoring an
  old body. If capture created the daily note, Undo may leave an empty note;
  it must not delete unrelated edits.

Two offline devices capturing different conversations into the same daily
note still produce the existing whole-body conflict. This proposal does
not introduce an append CRDT or promise automatic text merging.

### Explicit links and backlinks

The note editor's **Link to…** action offers **Conversation**, **Task**,
and **Event**, with local search and account/calendar labels so identical
names are distinguishable. Selection inserts an ordinary Markdown link
with an application destination. Existing `[[note links]]` keep their
current meaning. Mail capture uses the same reference builder.

| Target | Durable reference |
| --- | --- |
| Conversation | Account identity plus provider conversation identity, resolved through the existing mail identity mappings; never a device-local database row alone |
| Existing task | Replicated task id |
| Note task | Owning note id plus stable task marker id, described below |
| Event | Account/calendar identity plus provider event identity; include recurrence identity for a specific occurrence |

An event picker must distinguish **This occurrence** and **Entire series**
for a recurring event. Rescheduling an occurrence must preserve its link;
its current start time alone is not an identity. A conversation's focused
message may be an optional navigation anchor, with conversation fallback if
that message is unavailable. Do not guess a different conversation from a
subject, sender, or matching Message-ID header.

Use `threestrands:conversation/…`, `threestrands:task/…`, and
`threestrands:event/…` destinations, with versioned, strictly validated,
percent-encoded reference payloads. These are inert Markdown references,
not OS deep-link registration. The native resolver maps account identities
on this device using established mappings; an unknown account or target
stays unavailable until it can be resolved safely.

- Handle recognized destinations inside Notes before the external-link
  opener. Dispatch to typed navigation callbacks; never navigate the webview
  or send them to the OS. Keep the external `http`/`https`/`mailto`
  allowlist. Reject malformed references, unknown versions and kinds,
  extra fields, and oversized payloads. Never interpret a reference as a
  command, file path, SQL fragment, or remote URL to fetch.
- A missing, deleted, disconnected, or not-yet-synchronized target keeps
  its authored label and shows **Unavailable on this device** on activation.
  Linking does not fetch mail, connect an account, or create the target.
  Target title changes do not rewrite authored labels.
- Parse links outside escaped text, code, and inert metadata. Build a
  local `note_entity_links` index on every body projection, including
  imports and conflict resolutions. Deduplicate by note and canonical
  target for backlinks; retain occurrences for navigation. Derive these
  rows from Markdown rather than synchronizing a second relationship list.
- Notes' context panel adds **Conversations**, **Tasks**, and **Events**
  sections for explicit outgoing references. Mail, task detail, and event
  detail add **Notes** backlinks showing the note title and referring
  excerpt. Exclude notes in Trash; restore rebuilds their visibility.
  Use the existing context rail components, collapse behavior, and
  three-row **Show more** limit. Keep explicit links distinct from
  automatically suggested related context.
- Opening a backlink selects its occurrence in the note. A title rename
  does not break entity links. Deleting a target leaves a broken link;
  unlinking never deletes either item.
- Export keeps these destinations, labels, and task ids verbatim. Other
  Markdown apps can read their text but cannot open application references.
  Reimport preserves references without inventing missing accounts or
  targets. Notes and relationships remain outside settings export.

### Markdown checklists and tasks

Follow [Reflect's task/checklist distinction](https://reflect.academy/tasks-beta):
checklists are note-local items, while tasks are actionable items collected
in the Tasks workspace. Use different shapes and explicit creation actions.
The parenthesized Markdown below is ThreeStrands' format choice; Reflect's
page describes its UI and shortcuts, not this storage syntax.

| Kind | Unchecked Markdown | Checked Markdown | Editor appearance | Appears in Tasks |
| --- | --- | --- | --- | --- |
| Checklist | `- [ ] Item` or `- [] Item` | `- [x] Item` | Square checkbox | No |
| Task | `- ( ) Item` or `- () Item` | `- (x) Item` | Round checkbox | Yes |

```markdown
- [ ] Milk
- [x] Bread
- ( ) Send the proposal
- (x) Confirm the venue
```

`[X]` and `(X)` also mean checked. Accept unordered and ordered list
items, including nested items. Require whitespace between the closing
marker and nonempty item text. Parentheses elsewhere are ordinary prose;
`[label](destination)` remains a link, and code, escaped markers, empty
items, and blockquotes create neither checklist items nor tasks. Each
nested item is independent, including mixed checklist/task lists; checking
or converting a parent never changes its children. The task title is the
item's first-line inline text, with Markdown formatting removed. Following
paragraphs and child items remain note content, not part of that title.

The editor's slash menu offers **Checklist** and **Task**, inserting the
corresponding syntax. Each item's context row offers **Convert to Task**
for a checklist and **Convert to Checklist** for a task. Tasks also offers
**Convert to Checklist** for note tasks; other task sources keep their
existing actions. Conversion changes only the item's marker family and
preserves text, checked state, indentation, children, links, and any
persisted task id. A checked checklist becomes a completed task.

Saving a conversion to Task adds exactly one task to the projection and
normal task queries. Saving a conversion to Checklist removes it from
Tasks, including counts, search, and Completed, without deleting the
item from its note. Keep any existing task-id metadata inert on a checklist
so conversion back or Undo restores the same task identity. An old task
link to a converted checklist opens that item in its note with a
**Checklist** indicator; it does not recreate a task. Fresh checklists need
no task id, and neither reading nor checking them assigns one. Conversion
uses the same guarded save and draft handling as other note mutations.

Preserve the marker family when toggling: `[]` may become `[x]`, and `()`
may become `(x)`; toggling never converts a checklist into a task. The
editor may normalize spacing only after an actual edit, with documented
round-trip fixtures. Both kinds use accessible checkbox semantics in
Hide/Hybrid mode, with square versus round styling and accessible labels
that identify the kind. Show mode preserves their distinct source syntax.
This requires extending the editor's parser/serializer, not a regex
replacement of the full body. Existing and imported standard Markdown
checkboxes remain checklists; do not automatically promote them to tasks.

**One source of truth.** A note task's title, completion, and existence
come from the saved Markdown body. Project them into a local `note_tasks`
table in the same transaction as the note; do not create independent
replicated `Task` entities for these round task items. This avoids two sync
fields disagreeing about whether one task is done. Unsaved typing may
preview a task in the editor but does not enter Tasks until saved. Square
checklist items never enter `note_tasks`, even when they retain an inert
task-id marker after conversion.

**Stable identity.** At the first authored save/import, attach an inert
trailing marker to each new task item, for example
`<!-- threestrands-task:<uuid> -->`. It is metadata text, never live HTML.
The editor keeps this metadata hidden in every syntax mode and preserves
it byte for byte; export exposes the stored Markdown source. Moving, rewording, or
checking the item preserves its id. Copying an item within a note assigns
a fresh id to the copy on save; the first occurrence keeps the original.
Identity is `(noteId, markerId)`, so copying to a different note creates a
distinct task. Reject malformed marker ids as metadata; do not execute or
render arbitrary comments or HTML.

Existing unmarked parenthesized tasks must appear without rewriting a note
on read. Unmarked square checklist items remain excluded.
Give them deterministic provisional ids derived from note id, first-line
source text without the checkbox state, and occurrence among identical
items. On their first mutation/save, persist that same id as a marker.
Reordering or editing an unmarked item can change provisional identity;
there is no stored task metadata to lose in this first release. Do not
create durable links to provisional ids: creating such a link first saves
the marker through the guarded mutation path. Two devices promoting the
same unchanged item derive the same id. An imported duplicate marker is
repaired during the explicit import/save, never during passive projection.

**Tasks integration.** Extend the task query and UI model with a
`source: mail | standalone | note` discriminator rather than pretending
notes belong to a mail account. Existing mail/standalone task storage and
account filtering retain their meaning.

- Note tasks appear in **All** and **Anytime**, or **Completed**, in list
  and board layouts, with a **Note · <title/date>** source link. Include
  them in task counts and existing text search. Account-specific task
  views exclude them; the all-accounts view includes them, even with no
  mail accounts connected. Linking a note to mail does not assign its
  tasks to that mail account.
- A daily note's date is context, not a due date. Do not put its undated
  tasks in **Today**, **Upcoming**, or **Overdue** just because of
  the note's date. First release note tasks support open/completed only;
  due dates, goals, waiting/follow-up, in-progress, recurrence, delegation,
  and automatic reply completion require a later metadata design. Hide
  unsupported task actions, including dropping a note task into
  **In Progress**; keep existing tasks' full behavior.
- Checking in Tasks patches only the owning Markdown marker, preserving
  surrounding text. Changing the title is done in the note through
  **Open Note**. The source link focuses that item. A Markdown link to
  an existing task shows a reference, not a second task row; checking a
  checklist or task item containing that link checks that item only,
  without completing the referenced task.
- Route changes through the note's draft/save owner. Flush an open draft
  before applying a version-checked patch, then update the editor from the
  committed result. On stale, conflicted, trashed, deleted, or unsavable
  bodies, keep both views unchanged, preserve drafts, and offer **Open
  Note** to resolve. Do not silently retry against a different item.
- Note tasks disappear when their item is removed or the note goes to
  Trash, and return on Undo/restore with the same persisted ids. Permanent
  deletion removes the local projection. They are never silently converted
  to standalone tasks. Conflicted notes show the chosen working body's
  tasks with a conflict indicator and disabled mutations until resolution;
  rebuilding from the selected version removes losing-version tasks.
- Completion time is local derived history in this first release, not a
  fabricated synchronized timestamp. Completed note tasks appear in
  **Completed**; order them by locally observed completion, with stable
  note/task-id tie breaks. Rebuilding may lose that local history.

### Storage, compatibility, and limits

Use one native Markdown parser for checklist/task kinds and ranges, marker
ids, explicit links,
and patching. Share fixtures with the editor so rendered tasks and indexed
tasks agree. Body projections update `notes`, search, wiki links, entity
links, and note tasks atomically. A failed save leaves all of them unchanged.
Opening Notes or Tasks never normalizes or saves Markdown. Marker bytes
count toward existing note and total limits; an over-limit insertion keeps
the draft and reports the usual error without truncation.

Put new numeric bounds in `crates/sync-protocol` beside note limits:
`MAX_NOTE_TASKS` = 500 round task items per note (square checklist items
do not count toward this task limit), `MAX_NOTE_ENTITY_LINKS` = 500
entity-link occurrences per note, and `MAX_NOTE_REFERENCE_BYTES` = 2,048
UTF-8 bytes per encoded destination. Test below, exactly at, and above
each bound. Task text remains governed by the note body limit; do not
apply the existing standalone-task title limit or truncate the stored text.
Never index only a silent prefix of a saved note: refuse a new over-limit
edit/import with an explanation. Existing or received over-limit content
stays readable, with an explicit indexing warning and no partial task list.

This first release adds local derived tables and extends Markdown syntax;
it adds no replicated entity type or settings-transfer field. Older devices
may edit a note using their older editor and strip unknown markers or
normalize the parenthesized task syntax. The previous pinned editor's
round-trip fixture verifies preservation of both the parentheses and task
markers. Keep that compatibility fixture when upgrading the editor; an
upgrade that cannot preserve them needs an explicit compatibility gate.

### Acceptance and delivery

1. **References and capture.** Ship the validated reference codec, native
   identity resolution, Markdown index, guarded daily append, Mail action,
   and **Open Note**/**Undo** behavior together.
2. **Navigation and backlinks.** Add the picker, typed navigation, and
   context sections across Notes, Mail, Tasks, and Calendar. Cover event
   occurrences and unresolved targets before enabling the event picker.
3. **Markdown checklists and tasks.** Ship distinct parser/editor kinds,
   square/round rendering, creation and conversion actions, stable task
   markers, native projection, unified task queries/counts/search, and
   guarded task writes in both views together. Keep checklists out of Tasks
   throughout; do not advertise round tasks before aggregation works.

Automated coverage must include:

- Capture into missing/existing daily notes, device-local midnight, repeated
  capture, fenced-code tails, Undo after further edits, and preservation of
  focused mail message and expansion state. No full message content copied.
- Dirty open/closed drafts, stale versions, Trash, deletion races, size
  failures, and two offline captures producing a recoverable body conflict.
- Account collisions, missing identities, rebuilds, task links, recurring
  series versus occurrences, reschedules, and backlinks after rename,
  Trash/restore, import, and conflict resolution.
- Distinct checklist/task marker families, square/round rendering,
  accessible kind labels, slash-menu creation, case/spacing, ordered and
  mixed nested items, duplicate text, copied/moved tasks, deterministic
  marker assignment on two devices, and
  negative fixtures for code, escaping, blockquotes, prose parentheses,
  Markdown links, empty items, forged ids, and hostile HTML/destinations.
- Checklist toggles never create tasks or ids; existing/imported square
  checkboxes stay checklists and never affect task counts or search.
  Both conversion directions preserve text, checked state, children,
  links, and existing ids; checked-to-task enters Completed. Conversion
  to checklist removes the task from every query, count, and board;
  conversion back and Undo restore its identity. Existing task links
  navigate to converted checklists without recreating tasks. Failed or
  conflicted conversions leave the body and Tasks unchanged.
- The same round task appears once and toggles in both directions; all-accounts
  and account-specific filtering, counts, search, Completed/Anytime,
  list/board parity, unsupported actions, no invented due dates, conflict
  read-only behavior, and item removal/restoration.
- Editor load-without-write and marker-preserving round trips, export/import,
  old-editor compatibility, parser boundaries, and atomic failure leaving
  body and projections unchanged. Security-negative fixtures must prove
  internal links cannot escape into OS/webview navigation or remote fetches.

Test parsers and storage natively, hooks/components directly, and retain
slim App tests for capture wiring and cross-workspace return navigation.
Run the required frontend, Rust, and mail-triage E2E checks when changing this
behavior, and keep the user-facing changelog entry current.

### Implementation notes

- Native parsing and source patches live in `note_entities.rs`; `db/note_entities.rs`
  owns the local task, entity-link, warning, and cached-event tables (migration
  104). Note saves/imports and replicated projection rebuild those indexes
  with the note. Existing unmarked tasks are indexed during migration without
  changing their bodies or publishing edits.
- The editor adapts the stored parenthesized tasks to meowdown's existing
  round-checkbox representation. Identity comments are hidden block atoms in
  the editing view and return to their original trailing position on save.
  Shared Markdown and reference-codec fixtures, previous-editor compatibility,
  and browser coverage protect that boundary, including adding a second task.
- **Link to…** searches cached conversations, existing tasks, and events already
  loaded in Calendar. Event creation, edits, deletion, and successful Calendar
  range loads maintain the local event cache. Following a note link never
  starts a provider request; an event outside the cache needs its dates loaded
  in Calendar first. Series links resolve to an available cached occurrence.
  Removing a calendar account clears its cached events while preserving note links.
- Notes' context panel has one **Links** section, rather than separate
  **Conversations**, **Tasks**, and **Events** sections. **Link to…** sits
  in that section and in the command palette (**Link to Conversation, Task,
  or Event…**). In **Checklists and tasks**, each row converts with one
  small icon button rather than a **Convert to …** link.
- Task scope controls include **All accounts** even with one mail account.
  Changing the scope while in Tasks keeps Tasks open. Note tasks join the
  all-accounts query; account-specific queries continue to exclude them.
- Task mutations coordinate with the open editor's save owner, advancing the
  expected version only through that owner's own successful flush. Closed
  drafts, external changes, conflicts, and limit failures require resolution.
  Conversions keep each row mounted while its refreshed projection loads, so
  keyboard focus is retained.

## Meeting notes proposal

**Status: built.**
Notes for a calendar event are among the
most common notes people write. Today a user can link a note to an event
(**Link to… → Event**), and the event's **Meeting context** lists the notes
that link to it. But there is no way to start a note from the event itself.
This proposal adds that, following
Reflect's calendar flow: an **Add Note** button on
each event writes one line about the meeting into that day's note, and can
also link a note named after the meeting. Attendees link to their contact
records.

### User contract

- From any event in an agenda, record the meeting in that day's daily note
  without leaving the view.
- The line says when the meeting was, who was there, and what it was. The
  time opens the event, each attendee opens their contact, and the meeting
  name can open a note shared by every occurrence of that meeting.
- A contact's record lists every note that mentions them, so a person's
  meeting history is one click away.
- Nothing is copied from the event beyond its title, start time, and
  attendees. The event's description, location, and conference link stay out
  of the note.

### Add Note

**Entry points.** An **Add Note** button on each event row in the agendas
(`CalendarAgenda` in Calendar's rail and in Today, and Today's next-meeting
card), in the event's **Meeting context**, and the palette command **Add
Meeting to Daily Note**, enabled when an event is selected. No default
keyboard shortcut in this release.

**The dialog** opens over the current view, with focus in **Meeting name**:

- A header showing the event's start time (`formatClockTime`), or **All
  day**.
- **Meeting name**, filled with the event's title. Changing it changes only
  the note text, never the event.
- **Attendees**, as removable chips, filled from the event's attendees
  without the user's own account addresses. Each chip shows the saved
  contact's name when one owns the address, else the address. **Add
  attendee** searches contacts like a recipient field, and accepts a typed
  address that no contact has.
- **Create backlinked note**, a checkbox, off by default. When checked, the
  dialog shows the note title it will link (see [Meeting note](#meeting-note)),
  and says whether that note exists or will be created.
- **Cancel** and **Add to Daily Note**. Enter submits; Escape cancels.

The dialog never changes the event and never contacts a provider.

### The daily-note line

The line goes into the daily note for the event's start date in the device's
local time zone, which may not be today. An event in the past or the future
writes to its own day's note.

```markdown
## Meetings

- [9:30am](threestrands:event/…) met with [Pete B](threestrands:contact/…), [Mike Carpenter](threestrands:contact/…), [Frank Jackson](threestrands:contact/…), [Mark Williams](threestrands:contact/…) for CS Leadership Meeting (and Sales Mtg)
```

- **Time.** The start time, formatted as the agenda shows it, links to the
  occurrence (the event reference already used by **Link to…**, including
  its recurrence identity). An all-day event's link text is **All day**.
- **Attendees.** Each is a [contact reference](#contact-references), in the
  dialog's order, separated by commas as Reflect writes them. With no
  attendees left, the line drops `met with … ` and reads `- [9:30am](…) for
  <name>`.
- **Meeting name.** Plain text, or, with **Create backlinked note**, a
  `[[Title]]` link to the [meeting note](#meeting-note).
- Link labels and the plain meeting name escape Markdown punctuation, and
  line breaks become spaces, as **Add to Today's Note** does.

**Placement.** The line is appended to the note's `## Meetings` section: the
first `## Meetings` heading (matched by normalized text) outside code, up to
the next heading of the same or a higher level. It goes after the section's
last non-blank line. Without such a heading, `## Meetings`, a blank line,
and the line are appended at the end of the note, separated from existing
text by a blank line. Lines are not reordered by time in this release.

**Repeats.** If the note already links this occurrence anywhere (the same
canonical event reference, not the same text), nothing is added, and the
notice says **Already in the note** with **Open Note**. A line the user
typed linking the same event counts.

**Saving** reuses the guarded capture path of **Add to Today's Note**,
generalized from "append a conversation to today's note" to "insert lines
into a section of a given day's note" (`capture_note_conversation` becomes
one caller of the shared insert):

- An open or closed unsaved draft of that day's note is saved first, or the
  action is refused and the draft is kept.
- The write is a version-checked native transaction. A stale version, a body
  conflict, Trash, a permanent deletion racing the write, an unclosed code
  fence at the insertion point, or the size limits refuse the whole action,
  with **Open Note**. Nothing is written in part, and a deleted note is
  never recreated silently.
- On success the user stays where they were. The notice says **Added to
  <weekday>'s note** (or **today's note**), with **Open Note** and **Undo**.
  **Open Note** selects the inserted line.
- **Undo** removes exactly the inserted line, and the `## Meetings` heading
  if this action added it, under the same version check. It also moves a
  meeting note this action created to Trash, if that note is still empty
  and unchanged. If either note changed since, Undo offers to open it
  instead.

Two offline devices adding meetings to the same day still produce the
existing whole-body conflict for that daily note. This proposal adds no text
merging.

### Meeting note

With **Create backlinked note** checked, the meeting name becomes a
`[[Title]]` link, and the note with that title is created if it doesn't
exist. Recurring meetings use the same title each time, so every occurrence
links the same running note, whose backlinks list each day's line.

- **Title.** The meeting name, made valid with the existing title rules:
  `[`, `]`, and `|` become `-`, repeated spaces collapse, and the title is
  cut to 200 characters at a character boundary. A name that is a date gets
  " (meeting)". An empty name uses **Meeting**.
- **Existing note.** A note outside Trash whose title matches by normalized
  key is linked, not duplicated. That is the expected case from a meeting's
  second occurrence on.
- **New note.** Created in the same transaction as the daily-note line, with
  an empty body, so it appears in Recent and search. Two devices that create
  the same meeting note offline get the existing title rule: the lowest id
  keeps the title, the other gets a number, and `[[Title]]` links resolve to
  the one keeping it.
- The meeting note holds no event link of its own; the event is reached from
  the daily line, and the event's **Notes** backlinks list the daily note.

### Contact references

A fourth kind of note reference, beside `conversation`, `task`, and
`event`, with the same codec, strict validation, 2,048-byte limit
(`MAX_NOTE_REFERENCE_BYTES`), and entity-link index:

```json
{ "version": 1, "kind": "contact", "id": "pete@example.com", "contact": "<contact id>" }
```

- **`id` is the attendee's address**, normalized as contact lookup
  normalizes it. **`contact`** is the saved contact's id when there was one,
  and is optional. No other fields are accepted for this kind.
- **Why the address.** Merging contacts deletes the merged-away contacts
  (`merge_contacts`), so a contact id alone would break; the merge moves
  addresses to the contact it keeps. The address is also all an event
  gives for someone not yet saved.
- **Resolving.** Following a link opens the contact by id when it still
  exists, else the saved contact that now owns the address, in **Contacts**.
  With neither, it shows **No contact for <address> on this device**. It
  never creates a contact, sends mail, or fetches anything.
- **Labels** are the name written at insertion and don't change when the
  contact is renamed, like other references.
- **Inserting links.** In the Markdown editor, a standalone `@` opens
  autocomplete for **Contacts** and **Mail**. Search contacts by name or
  address and conversations by subject or participant. Results are grouped
  by type, with contact addresses and conversation participants, account,
  and date to distinguish similar matches. Contacts share the Contacts view's
  ordering: favorites first, then most recent correspondence across accounts.
  Conversations are newest first. Choosing one inserts the person's
  name or conversation subject as a standard Markdown application link.
  **Link to…** also offers **Contact** alongside its existing choices, and
  **Add Note** continues to write attendee references.
- **Separate note links.** `[[` autocomplete searches notes and can name a
  new note. `@` does not offer notes, tasks, or events. An `@` inside an
  ordinary email address, or inside code, stays literal. Autocomplete uses
  local data, supports keyboard selection and Escape, and ignores stale
  search results when the query or open note changes. No new reference
  format or sync entity is introduced.

**Backlinks.** The entity-link index keys a contact reference by its address,
so references written before and after a merge, or with and without a
contact id, are one target. The contact's **Contact context** in Contacts
gets a **Notes** section (`NoteBacklinks`), queried for every address the
contact has, with the referring line as each row's detail. Notes in Trash
are left out, as elsewhere.

**People section.** Notes' **People** section lists contact references'
contacts as well as `[[Name]]` links that match a contact's name,
deduplicated by contact. Contact references are not `[[links]]`, so they
never appear under **Unlinked**.

**Privacy.** Attendee addresses become note text. They sync encrypted with
the note and are written verbatim by **Export to Markdown Folder**, as every
reference is. That fits notes being the user's own content; the dialog
lets the user remove any attendee before adding.

### Compatibility

No new entity type or settings-transfer field. A device on 0.208.0 parses a
`contact` reference as an unknown kind: it shows the label and refuses to
follow it, and its editor must keep the text unchanged. The previous
editor's round-trip fixture gains a contact reference before this ships.
The `## Meetings` heading and the line are ordinary Markdown that every
version and other Markdown apps read.

### Delivery

1. **Contact references.** The codec kind, native parsing and the address
   key in `note_entity_links`, resolution and navigation into Contacts, the
   **Notes** section on contacts, and People.
2. **Section insert.** The shared native insert (section lookup, fence
   check, repeat check, version check, optional meeting-note creation) and
   Undo, with **Add to Today's Note** moved onto it unchanged.
3. **Add Note.** The dialog and its entry points, wiring, and the changelog
   entry.

Automated coverage:

- Codec: valid contact references; missing or extra fields; uppercase or
  malformed addresses; below, at, and above the reference byte limit.
- Resolution: by id; by address after the id was merged away; an unsaved
  address; a contact deleted; backlinks shared across merged contacts and
  across references with and without an id.
- Line text: punctuation escaping, line breaks, all-day events, no
  attendees, the user's own addresses excluded, attendees added by address.
- Placement: an existing section in the middle of a note, at the end, a
  `## Meetings` inside a code fence ignored, no section, an empty note, a
  missing daily note, and a past and a future date. Repeats by canonical
  event, including a user-typed link.
- Meeting note: new title, existing title, title sanitizing (brackets, pipe,
  a date, 200-character cut), a same-titled note in Trash, and two devices
  creating it offline.
- Refusals leave both notes and all indexes unchanged: open and closed
  drafts, a stale version, conflict, Trash, permanent deletion, size limits.
  Undo after further edits offers to open the note.
- Security-negative: contact references never navigate the webview, open a
  `mailto:`, or fetch; a hostile address or label renders as text.
- Old-editor round trip of contact references, and a slim App test for the
  agenda button and the palette command reaching the dialog.

### Implementation notes

Step 1, as built:

- `note_entity_links` gained a `target` column (migration 105): the link's
  `NoteReference::target()`, which is its href for every kind but a contact
  reference, whose target drops the contact id. Backlinks query by target and
  return each note's first matching link with its own href, so opening the
  row finds that exact text. Migration 105 indexes every note again, which
  also indexes contact references a device received before it understood
  them; migration 104 no longer indexes, since 105 always follows it.
- `resolve_note_contact` opens the saved contact by id, then the saved
  contact that owns the address, then, beyond the proposal, the contact the
  app derives from mail the user sent to that address
  (`derived:<address>`). Only with none of those does the link say **No
  contact for <address> on this device**.
- Contacts' **Contact context** lists the Notes section for all of the
  contact's addresses, and now shows for a contact with notes but no email
  and no AI features (`useNoteBacklinks`).
- People resolves at most `PEOPLE_LOOKUPS` (30) contact references and title
  links together, contact references first.
- Nothing writes contact references yet, so users see no change until step
  3; the changelog entry comes with it.

Step 2, as built:

- `note_meetings.rs` writes the line, finds the section, and makes the
  meeting note's title, as pure functions over the stored text;
  `src/noteMeetings.ts` mirrors them for the demo client.
  `fixtures/note-meetings.json` holds both to the same placement, titles,
  and lines.
- `add_meeting_to_daily_note` and **Add to Today's Note** share one start
  (`begin_daily_insert`: date, version, Trash, and conflict checks, then the
  parse). Add to Today's Note's text, messages, and repeat rule are
  unchanged. A meeting's messages say "that day's note", since the event's
  day may not be today.
- Link labels and a plain meeting name escape the same characters as
  `markdownEntityLink`, so `(and Sales Mtg)` is stored as
  `\(and Sales Mtg\)`; it reads the same once rendered. A line names at
  most 50 attendees (`MAX_MEETING_ATTENDEES`), and the time label is at most
  32 characters.
- **Undo** (`addMeetingToDailyNote` in `noteCapture.ts`) restores that day's
  note to its text before the action, under the same version check as Add to
  Today's Note's Undo, which removes exactly the line and any heading it
  added. It then moves a meeting note the action created to Trash only if it
  is still empty and at its created version.

Step 3, as built:

- `MeetingNoteDialog` is titled **Add Note**. The attendee chips are the
  event editor's `EmailBadgeField`, which now takes names from outside
  (`knownNames`) and reports names chosen in it (`onNamesChange`). Names come
  from contacts and the people the user emails; the user's own account and
  calendar addresses are left out. A saved contact's id goes in the reference;
  derived ids don't.
- Add Note appears on agenda rows (Calendar's rail and Today), Today's
  next-meeting card, and the open event's **Event details** heading. Events
  without a calendar id can't be linked and don't offer it. The palette's
  **Add Meeting to Daily Note** is enabled while Calendar has an event's
  details open (`addMeetingNote` in the command context).
- The notice says **Added to today's note** (or tomorrow's, yesterday's, or
  "the note for <date>"), with **Open Note** and Undo, or **Already in the
  note**. A refusal keeps the dialog open with its reason and **Open Note**.

### Recurring series

An occurrence's notes alone don't answer "what have we said in this weekly
meeting?" Each occurrence has its own event id, so its links don't find the
notes about the series' other occurrences. So an event reference can also
name its series:

```json
{ "version": 1, "kind": "event", "id": "<occurrence id>", "account": "…", "calendar": "…", "series": "<series id>" }
```

- **Writing.** Add Note and **Link to… → This occurrence** record the
  event's `seriesId` when it has one other than its own id. **Entire
  series** links name the series as their `id`, without `series`.
- **Validation.** `series` is accepted only on event references and never
  equals `id`. It is a hint like a contact reference's `contact`: the
  occurrence's target leaves it out, so an occurrence linked with and
  without it is one target.
- **Index.** `note_entity_links.series` (migration 106) holds the target of
  the series an event link counts toward: the link's own `series`, else the
  series this device's event cache knows for that occurrence. Loading dates
  in Calendar fills in links written before their series was known
  (`cache_note_events`). Migration 106 indexes every note again; 105 no
  longer does, since 106 always follows it.
- **Backlinks.** A query for the series matches links whose target or
  series is the series, so it lists notes about the series as a whole and
  about any occurrence. The event's context panel shows **Notes** (this
  occurrence) and **Series notes** (the rest of the series, newest day
  first).
- **Compatibility.** Earlier versions accept no `series` field, so on a
  device not yet updated an occurrence link written this way shows its text
  but can't be followed, as with contact references. Occurrence links
  written before this change keep working everywhere.

### Opening an event's existing note

The Event details action says **Open Note** when the selected occurrence has
an active linked note, otherwise **Add Note**. All calendar Add Note entry
points recheck occurrence backlinks before showing the dialog and open an
existing note instead. A dedicated meeting note takes precedence over a daily
note; existing additional links stay available in **Notes**. Notes about other
occurrences stay in **Series notes** and do not prevent adding this occurrence.
Neither section searches for notes merely mentioning an attendee.

### Open questions

- Should the agenda's button read **Open Note** when that day's note already
  links the occurrence? That needs a per-row lookup against the entity-link
  index; this proposal checks on click instead.
- Should lines in `## Meetings` be kept in time order?
- Should **Link to…** offer contacts once contact references exist?

## Later

Nothing below is planned for the next release.

- **v2: Folder mirror.** Keep a chosen folder in step with notes in both
  directions, for users who edit in other tools.
- Tags beyond plain-text search.
- Asking AI about notes, with a private flag per note. See the sketch
  below for summaries of a recurring meeting.
- Merging concurrent text edits.
- Note tasks with due dates, waiting, recurrence, and the other task states
  (see [Tasks integration](#markdown-checklists-and-tasks)).
- Following a link to an event outside the local event cache without first
  loading its dates in Calendar.
- Splitting snapshots by entity type, which would lift the 3.5 MiB notes
  limit (see [Size limits](#size-limits)).
- A continuous journal: the daily notes as one scrolling page, as in
  Reflect, skipping days without text other than today. The editor, save
  state, and conflict banners are built for one open note, so days other
  than the open one would show as read-only text that becomes the editor
  when chosen.

### Sketch: summarizing a meeting series

Thirty notes for one weekly meeting are less useful than a view of its
themes, decisions, and open issues. The data to build one is in place: a
series lists every note about it ([Recurring series](#recurring-series)),
each meeting line has its date, attendees as contact references, and its
place in the note, and note tasks carry their open or done state. A design
should settle these before building:

- **What is sent.** Only each meeting's own part of a note (its list item
  and the items nested under it, or the meeting note), not the whole daily
  note, which holds unrelated writing. Open and done note tasks from those
  parts go with them. The user sees which notes and how much text before
  anything is sent.
- **Consent and privacy.** Its own AI feature switch and provider choice,
  like the existing AI features, and a per-note private flag (Reflect's
  `private: true`) that keeps a note out. Nothing runs in the background;
  each summary is asked for.
- **Where results go.** A summary is a proposal shown beside the series
  (themes, decisions, open issues with their tasks, people). Keeping it
  writes a dated `## Summary` section into the meeting note through the
  guarded save, so it syncs and can be undone like any edit. It never
  edits, merges, or deletes the meeting notes themselves.
- **Cleanup.** Suggestions such as "these open items repeat across six
  meetings; make one task" are offered one at a time and applied through the
  existing conversion and Trash paths, never as a bulk rewrite.
- **Size.** A long series is summarized in date-ordered batches, with the
  previous summary as context, so cost and prompt size stay bounded.
